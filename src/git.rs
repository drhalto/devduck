//! What git says about a project folder: branch, uncommitted changes, commits not yet
//! pushed or pulled, the last commit, and the repo's web page. Read with the git CLI
//! (no account, nothing leaves the machine); folders that aren't in a repo give None.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::win::CREATE_NO_WINDOW;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitInfo {
    /// Branch name, or the short commit when detached.
    pub branch: String,
    pub detached: bool,
    /// Files changed, staged or untracked.
    pub changed: usize,
    /// Commits to push / to pull; None without an upstream.
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub last_subject: String,
    /// Unix seconds of the last commit; 0 in a repo with no commits yet.
    pub last_time: u64,
    /// https page for the origin remote (GitHub, GitLab, Bitbucket...).
    pub web: Option<String>,
}

impl GitInfo {
    /// "main · 3 changed · 2 to push": the branch, then only what needs attention.
    pub fn summary(&self) -> String {
        let mut parts = vec![self.branch.clone()];
        if self.changed > 0 {
            parts.push(format!("{} changed", self.changed));
        }
        if let Some(n) = self.ahead.filter(|n| *n > 0) {
            parts.push(format!("{n} to push"));
        }
        if let Some(n) = self.behind.filter(|n| *n > 0) {
            parts.push(format!("{n} to pull"));
        }
        parts.join(" \u{00B7} ")
    }
}

fn git(folder: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn read(folder: &Path) -> Option<GitInfo> {
    let status = git(folder, &["status", "--porcelain=v2", "--branch"])?;
    let mut info = parse_status(&status);
    if let Some(log) = git(folder, &["log", "-1", "--format=%ct%x1f%s"])
        && let Some((t, s)) = log.trim_end().split_once('\u{1f}')
    {
        info.last_time = t.parse().unwrap_or(0);
        info.last_subject = s.to_string();
    }
    info.web = git(folder, &["config", "--get", "remote.origin.url"]).and_then(|u| web_url(u.trim()));
    Some(info)
}

fn parse_status(status: &str) -> GitInfo {
    let mut info = GitInfo::default();
    let mut oid = String::new();
    for line in status.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            info.branch = head.to_string();
        } else if let Some(o) = line.strip_prefix("# branch.oid ") {
            oid = o.chars().take(7).collect();
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            info.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok());
            info.behind = it.next().and_then(|b| b.trim_start_matches('-').parse().ok());
        } else if !line.starts_with('#') && !line.is_empty() {
            info.changed += 1;
        }
    }
    if info.branch == "(detached)" {
        info.detached = true;
        info.branch = if oid.is_empty() { "detached".into() } else { oid };
    }
    info
}

/// git@github.com:owner/repo.git, https://user@github.com/owner/repo.git → https://github.com/owner/repo
fn web_url(remote: &str) -> Option<String> {
    let rest = if let Some(r) = remote.strip_prefix("git@") {
        r.replacen(':', "/", 1)
    } else if let Some(r) = remote.strip_prefix("https://").or_else(|| remote.strip_prefix("http://")).or_else(|| remote.strip_prefix("ssh://git@")) {
        r.rsplit_once('@').map_or(r, |(_, h)| h).to_string()
    } else {
        return None;
    };
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let host = rest.split('/').next()?;
    (host.contains('.') && rest.contains('/')).then(|| format!("https://{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines() {
        let s = "# branch.oid 1234567890abcdef\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b src/x.rs\n? notes.txt\n";
        let g = parse_status(s);
        assert_eq!((g.branch.as_str(), g.changed, g.ahead, g.behind), ("main", 2, Some(2), Some(1)));
        assert_eq!(g.summary(), "main \u{00B7} 2 changed \u{00B7} 2 to push \u{00B7} 1 to pull");
        let d = parse_status("# branch.oid abcdef0123\n# branch.head (detached)\n");
        assert!(d.detached);
        assert_eq!((d.branch.as_str(), d.ahead), ("abcdef0", None));
    }

    #[test]
    fn remotes_to_web() {
        assert_eq!(web_url("git@github.com:drhalto/devduck.git").as_deref(), Some("https://github.com/drhalto/devduck"));
        assert_eq!(web_url("https://tok@github.com/drhalto/devduck.git").as_deref(), Some("https://github.com/drhalto/devduck"));
        assert_eq!(web_url("ssh://git@gitlab.com/a/b").as_deref(), Some("https://gitlab.com/a/b"));
        assert_eq!(web_url("C:/repos/local.git"), None);
    }
}
