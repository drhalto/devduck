//! The handful of Win32/NT calls Dev Ports needs, declared by hand with raw-dylib
//! (no bindings crate, no import libraries).
//!
//! The PowerShell version spent most of its ~1s scan in WMI (Win32_Process). Here a
//! single NtQuerySystemInformation call returns every process's pid, parent, name,
//! working set, CPU time and start time, and command lines / working directories are
//! read only for the processes the grouping actually looks at.

use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr::{null, null_mut};

type Handle = *mut c_void;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

/// SYSTEM_PROCESS_INFORMATION up to WorkingSetSize (layout from phnt; stable since Win7).
#[repr(C)]
struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: u64,
    user_time: u64,
    kernel_time: u64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
}

#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb_base_address: usize,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[link(name = "iphlpapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetExtendedTcpTable(table: *mut c_void, size: *mut u32, order: i32, af: u32, class: i32, reserved: u32) -> u32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret_len: *mut u32) -> i32;
    fn NtQueryInformationProcess(process: Handle, class: u32, info: *mut c_void, len: u32, ret_len: *mut u32) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn ReadProcessMemory(process: Handle, address: *const c_void, buffer: *mut c_void, size: usize, read: *mut usize) -> i32;
    fn GetSystemTimeAsFileTime(filetime: *mut u64);
    fn GetLocalTime(time: *mut SystemTime);
    fn GetCurrentProcessId() -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(Handle, isize) -> i32, lparam: isize) -> i32;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
    fn IsWindowVisible(hwnd: Handle) -> i32;
    fn GetWindow(hwnd: Handle, cmd: u32) -> Handle;
    fn GetWindowLongPtrW(hwnd: Handle, index: i32) -> isize;
}

#[link(name = "dwmapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: Handle, attribute: u32, value: *const c_void, size: u32) -> i32;
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ShellExecuteW(hwnd: Handle, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> Handle;
}

/// `CREATE_NO_WINDOW`, for child processes (docker, taskkill, dev servers) spawned from the GUI.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

pub struct Listener {
    pub port: u16,
    pub pid: u32,
    pub addr: IpAddr,
}

/// Every listening TCP socket (IPv4 first, then IPv6) with its owning pid and bind address.
pub fn tcp_listeners() -> Vec<Listener> {
    let mut out = Vec::new();
    // MIB_TCPROW_OWNER_PID (24 bytes) vs MIB_TCP6ROW_OWNER_PID (56 bytes)
    collect_listeners(2, 24, 8, 20, &mut out); // AF_INET
    collect_listeners(23, 56, 20, 52, &mut out); // AF_INET6
    out
}

fn collect_listeners(af: u32, row_size: usize, port_offset: usize, pid_offset: usize, out: &mut Vec<Listener>) {
    const TCP_TABLE_OWNER_PID_LISTENER: i32 = 3;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    let mut size = 0u32;
    unsafe { GetExtendedTcpTable(null_mut(), &mut size, 0, af, TCP_TABLE_OWNER_PID_LISTENER, 0) };
    for _ in 0..3 {
        let mut buf = vec![0u32; (size as usize).div_ceil(4).max(1)];
        size = (buf.len() * 4) as u32;
        let rc = unsafe { GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut size, 0, af, TCP_TABLE_OWNER_PID_LISTENER, 0) };
        if rc == ERROR_INSUFFICIENT_BUFFER {
            continue; // the table grew between calls
        }
        if rc != 0 {
            return;
        }
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), buf.len() * 4) };
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let count = count.min((bytes.len() - 4) / row_size);
        for i in 0..count {
            let row = &bytes[4 + i * row_size..4 + (i + 1) * row_size];
            let addr = if af == 2 {
                IpAddr::V4(Ipv4Addr::new(row[4], row[5], row[6], row[7]))
            } else {
                IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&row[0..16]).unwrap()))
            };
            out.push(Listener {
                port: u16::from_be_bytes([row[port_offset], row[port_offset + 1]]),
                pid: u32::from_le_bytes(row[pid_offset..pid_offset + 4].try_into().unwrap()),
                addr,
            });
        }
        return;
    }
}

pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub working_set: u64,
    /// FILETIME (100 ns ticks since 1601, UTC); 0 when unknown.
    pub create_time: u64,
    /// User + kernel time, 100 ns ticks.
    pub cpu_time: u64,
}

/// Every process on the machine, from one NtQuerySystemInformation call.
pub fn processes() -> Vec<ProcessEntry> {
    const SYSTEM_PROCESS_INFORMATION: u32 = 5;
    let mut buf: Vec<u64> = vec![0; 1 << 16]; // 512 KB, grown if needed
    let mut ok = false;
    for _ in 0..8 {
        let mut needed = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(SYSTEM_PROCESS_INFORMATION, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut needed)
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            let want = (needed as usize + (64 << 10)).div_ceil(8).max(buf.len() * 2);
            buf.resize(want, 0);
            continue;
        }
        ok = status >= 0;
        break;
    }
    if !ok {
        return Vec::new();
    }

    let base = buf.as_ptr().cast::<u8>();
    let mut out = Vec::with_capacity(512);
    let mut offset = 0usize;
    loop {
        // SAFETY: the kernel filled `buf` with a chain of SYSTEM_PROCESS_INFORMATION
        // records; each ImageName buffer points inside `buf`, which outlives this loop.
        let entry = unsafe { &*base.add(offset).cast::<SystemProcessInformation>() };
        let pid = entry.unique_process_id as u32;
        let name = if entry.image_name.buffer.is_null() {
            if pid == 0 { "System Idle Process".to_string() } else { String::new() }
        } else {
            let chars = unsafe { std::slice::from_raw_parts(entry.image_name.buffer, entry.image_name.length as usize / 2) };
            String::from_utf16_lossy(chars)
        };
        out.push(ProcessEntry {
            pid,
            parent_pid: entry.inherited_from_unique_process_id as u32,
            name,
            working_set: entry.working_set_size as u64,
            create_time: entry.create_time,
            cpu_time: entry.user_time.saturating_add(entry.kernel_time),
        });
        if entry.next_entry_offset == 0 {
            break;
        }
        offset += entry.next_entry_offset as usize;
        if offset + size_of::<SystemProcessInformation>() > buf.len() * 8 {
            break;
        }
    }
    out
}

struct OwnedHandle(Handle);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open_process(pid: u32, access: u32) -> Option<OwnedHandle> {
    if pid <= 4 {
        return None;
    }
    let handle = unsafe { OpenProcess(access, 0, pid) };
    (!handle.is_null()).then_some(OwnedHandle(handle))
}

/// A process's command line, or None if it's gone or we lack access (elevated or
/// another user's process while we're not admin, same as WMI would report).
pub fn command_line(pid: u32) -> Option<String> {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
    let handle = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut buf: Vec<u64> = vec![0; 512];
    for _ in 0..3 {
        let mut needed = 0u32;
        let status = unsafe {
            NtQueryInformationProcess(handle.0, PROCESS_COMMAND_LINE_INFORMATION, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut needed)
        };
        if status >= 0 {
            // SAFETY: on success the buffer starts with a UNICODE_STRING pointing into itself.
            let s = unsafe { &*buf.as_ptr().cast::<UnicodeString>() };
            if s.buffer.is_null() || s.length == 0 {
                return None;
            }
            let chars = unsafe { std::slice::from_raw_parts(s.buffer, s.length as usize / 2) };
            return Some(String::from_utf16_lossy(chars));
        }
        if needed as usize <= buf.len() * 8 {
            return None;
        }
        buf.resize((needed as usize).div_ceil(8), 0);
    }
    None
}

/// A process's current directory, read from its PEB (x64 layout). Used so Start and
/// Restart can re-run a dev server exactly where it was running.
pub fn current_dir(pid: u32) -> Option<String> {
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;
    let handle = open_process(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
    let read = |addr: usize, buf: &mut [u8]| -> bool {
        let mut got = 0usize;
        let ok = unsafe { ReadProcessMemory(handle.0, addr as *const c_void, buf.as_mut_ptr().cast(), buf.len(), &mut got) };
        ok != 0 && got == buf.len()
    };
    let mut pbi = std::mem::MaybeUninit::<ProcessBasicInformation>::zeroed();
    let status = unsafe {
        NtQueryInformationProcess(handle.0, 0, pbi.as_mut_ptr().cast(), size_of::<ProcessBasicInformation>() as u32, null_mut())
    };
    if status < 0 {
        return None;
    }
    let peb = unsafe { pbi.assume_init() }.peb_base_address;
    let mut ptr = [0u8; 8];
    if peb == 0 || !read(peb + 0x20, &mut ptr) {
        return None; // PEB.ProcessParameters
    }
    let params = usize::from_le_bytes(ptr);
    let mut us = [0u8; 16];
    if params == 0 || !read(params + 0x38, &mut us) {
        return None; // RTL_USER_PROCESS_PARAMETERS.CurrentDirectory.DosPath
    }
    let len = u16::from_le_bytes([us[0], us[1]]) as usize;
    let buffer = usize::from_le_bytes(us[8..16].try_into().unwrap());
    if len == 0 || len > 4096 || buffer == 0 {
        return None;
    }
    let mut raw = vec![0u8; len];
    if !read(buffer, &mut raw) {
        return None;
    }
    let chars: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let dir = String::from_utf16_lossy(&chars);
    let trimmed = dir.trim_end_matches('\\');
    Some(if trimmed.ends_with(':') { dir } else { trimmed.to_string() })
}

/// Current time as a FILETIME, comparable with `ProcessEntry::create_time`.
pub fn now_filetime() -> u64 {
    let mut ft = 0u64;
    unsafe { GetSystemTimeAsFileTime(&mut ft) };
    ft
}

/// Local wall-clock time as "HH:MM:SS".
pub fn local_clock() -> String {
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Open a URL or folder the way Explorer would (ShellExecute "open").
pub fn shell_open(target: &str) -> bool {
    let (op, file) = (wide("open"), wide(target));
    let rc = unsafe { ShellExecuteW(null_mut(), op.as_ptr(), file.as_ptr(), null(), null(), 1) };
    rc as usize > 32
}

/// Colour this process's top-level window frame to match the app (Windows 11 DWM):
/// dark mode, caption background, caption text and border. A no-op on Windows 10.
/// Returns false until the window exists.
pub fn style_title_bar(caption: [u8; 3], text: [u8; 3], border: [u8; 3]) -> bool {
    unsafe extern "system" fn find(hwnd: Handle, lparam: isize) -> i32 {
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
            // Visible, unowned and with a caption: the app window, not winit's event-target window.
            const WS_CAPTION: isize = 0x00C0_0000;
            let captioned = GetWindowLongPtrW(hwnd, -16) & WS_CAPTION == WS_CAPTION;
            if pid == GetCurrentProcessId() && IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, 4).is_null() && captioned {
                *(lparam as *mut Handle) = hwnd;
                return 0;
            }
        }
        1
    }
    let mut hwnd: Handle = null_mut();
    unsafe { EnumWindows(find, &mut hwnd as *mut Handle as isize) };
    if hwnd.is_null() {
        return false;
    }
    let colorref = |c: [u8; 3]| u32::from(c[0]) | (u32::from(c[1]) << 8) | (u32::from(c[2]) << 16);
    let set = |attr: u32, value: u32| unsafe { DwmSetWindowAttribute(hwnd, attr, (&value as *const u32).cast(), 4) };
    set(20, 1); // DWMWA_USE_IMMERSIVE_DARK_MODE
    set(34, colorref(border)); // DWMWA_BORDER_COLOR
    set(35, colorref(caption)); // DWMWA_CAPTION_COLOR
    set(36, colorref(text)); // DWMWA_TEXT_COLOR
    true
}
