//! A tiny HTTP probe: is this port a web server, what status does `/` return, how fast,
//! and what's the page's <title>? Raw HTTP/1.1 over a TcpStream, no client library.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum Probe {
    Http { status: u16, title: Option<String>, millis: u32, html: bool },
    /// Accepted the connection but didn't speak HTTP (databases, sockets, IPC).
    NotHttp,
    /// Didn't answer in time (dev servers compiling their first page do this).
    Timeout,
}

impl Probe {
    pub fn is_http(&self) -> bool {
        matches!(self, Probe::Http { .. })
    }
}

/// Connect over loopback, preferring the family the server actually bound.
pub fn probe(port: u16, addrs: &[IpAddr]) -> Probe {
    let v6_only = !addrs.is_empty() && addrs.iter().all(|a| a.is_ipv6());
    let host: IpAddr = if v6_only { Ipv6Addr::LOCALHOST.into() } else { Ipv4Addr::LOCALHOST.into() };
    let started = Instant::now();
    let Ok(mut stream) = TcpStream::connect_timeout(&SocketAddr::new(host, port), Duration::from_millis(400)) else {
        return Probe::NotHttp;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(2500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let request = format!(
        "GET / HTTP/1.1\r\nHost: localhost:{port}\r\nUser-Agent: DevPorts/2\r\nAccept: text/html,*/*\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return Probe::NotHttp;
    }
    let mut buf = Vec::with_capacity(16 * 1024);
    let mut chunk = [0u8; 8192];
    let mut first_byte: Option<Instant> = None;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                first_byte.get_or_insert_with(Instant::now);
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() >= 64 * 1024 || contains_ignore_case(&buf, b"</title>") {
                    break;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                if buf.is_empty() {
                    return Probe::Timeout;
                }
                break;
            }
            Err(_) => break,
        }
    }
    parse(&buf, first_byte.map_or(0, |t| (t - started).as_millis() as u32))
}

fn parse(buf: &[u8], millis: u32) -> Probe {
    let text = String::from_utf8_lossy(buf);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split_whitespace();
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else { return Probe::NotHttp };
    let Ok(status) = code.parse::<u16>() else { return Probe::NotHttp };
    if !version.starts_with("HTTP/") {
        return Probe::NotHttp;
    }
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let html = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("content-type:") && l.contains("html")
    });
    let title = if html { extract_title(body) } else { None };
    Probe::Http { status, title, millis, html }
}

fn contains_ignore_case(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

fn extract_title(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title>")?;
    let raw = &body[start..end];
    let decoded = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .replace("&ndash;", "\u{2013}")
        .replace("&mdash;", "\u{2014}")
        .replace("&middot;", "\u{00B7}");
    let title = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_and_title() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><head><title>\n  Rough &amp; Ready\n</title></head>";
        match parse(resp, 12) {
            Probe::Http { status, title, millis, html } => {
                assert_eq!((status, millis, html), (200, 12, true));
                assert_eq!(title.as_deref(), Some("Rough & Ready"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn json_and_non_http() {
        assert!(matches!(parse(b"HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\r\n{}", 1), Probe::Http { status: 404, title: None, .. }));
        assert!(matches!(parse(b"SSH-2.0-OpenSSH_for_Windows_9.5\r\n", 1), Probe::NotHttp));
    }
}
