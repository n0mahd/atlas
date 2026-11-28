//! Installing atlasd on another machine from the hub, over SSH (decision 0025).
//!
//! The hub runs the hub account's own `ssh` (Tailscale SSH or a key the owner added) and
//! on the far side the same command the Machines page shows. User input never reaches a
//! local shell: the target is checked strictly and passed as one argument after `--`.

use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;

/// How long a pairing code works.
pub const PAIRING_TTL: Duration = Duration::from_mins(15);
/// How long one install over SSH may take, from connecting to the last line.
const TIMEOUT: Duration = Duration::from_mins(3);
/// How long to wait for the last output after ssh exits (a process left holding the
/// pipe, such as an ssh control master, must not hold the page).
const GRACE: Duration = Duration::from_secs(2);
/// The most output kept (the end of it).
const OUTPUT_CAP: usize = 64 * 1024;
/// Lines of output shown when an install fails.
const TAIL_LINES: usize = 15;

/// The command that installs and pairs atlasd, run in a terminal on the machine.
#[must_use]
pub fn install_command(origin: &str, code: &str) -> String {
    format!("curl -fsSL {origin}/install | sh -s -- {code}")
}

/// An SSH address checked to be safe as an `ssh` argument: `host` or `user@host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target(String);

impl Target {
    /// Checks an address typed by the owner.
    ///
    /// # Errors
    /// A plain message when it isn't a `user@host` or `host`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let bad = || {
            format!(
                "\"{text}\" isn't an SSH address. Use the machine's name, such as laptop, or you@laptop."
            )
        };
        if text.is_empty() {
            return Err("Type the machine's SSH address, such as you@laptop.".to_owned());
        }
        if text.len() > 253 {
            return Err(bad());
        }
        let (user, host) = match text.split_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, text),
        };
        let user_ok = user.is_none_or(|u| {
            let mut c = u.chars();
            c.next().is_some_and(|f| f.is_ascii_lowercase() || f == '_')
                && c.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || "_.-".contains(ch))
        });
        let mut h = host.chars();
        let host_ok = h.next().is_some_and(|f| f.is_ascii_alphanumeric())
            && h.all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-');
        if user_ok && host_ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(bad())
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What came back from a finished install: everything it printed (the end, if long).
#[derive(Debug)]
pub struct Outcome {
    pub output: String,
}

/// Why an install didn't finish, in plain words, with the end of its output.
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub output: String,
}

/// Installs and pairs atlasd on `target` with a fresh pairing `code`.
///
/// # Errors
/// A [`Failure`] saying what went wrong, for the owner to read.
pub async fn install_over_ssh(
    origin: &str,
    code: &str,
    target: &Target,
) -> Result<Outcome, Failure> {
    run(OsStr::new("ssh"), origin, code, target, TIMEOUT).await
}

/// The arguments given to `ssh`.
fn ssh_args(origin: &str, code: &str, target: &Target) -> Vec<String> {
    [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "--",
        target.as_str(),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain(std::iter::once(install_command(origin, code)))
    .collect()
}

/// Characters an origin or code may have, so the remote shell reads them as plain words.
fn shell_safe(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ":/._-[]".contains(c))
}

/// `install_over_ssh` with the program and time limit given, so tests can use a fake ssh.
pub(crate) async fn run(
    program: &OsStr,
    origin: &str,
    code: &str,
    target: &Target,
    timeout: Duration,
) -> Result<Outcome, Failure> {
    let fail = |message: String, output: String| Failure { message, output };
    if !shell_safe(origin) || !shell_safe(code) {
        return Err(fail(
            "The hub's address or the pairing code has characters a command can't carry."
                .to_owned(),
            String::new(),
        ));
    }
    let spawned = (|| {
        let (reader, writer) = std::io::pipe()?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(ssh_args(origin, code, target))
            .stdin(Stdio::null())
            .stdout(writer.try_clone()?)
            .stderr(writer)
            .kill_on_drop(true);
        let child = cmd.spawn()?;
        // Dropping `cmd` closes the hub's copies of the pipe's write end, so reading ends
        // when ssh (and whatever it started) has finished writing.
        drop(cmd);
        let rx = tokio::net::unix::pipe::Receiver::from_owned_fd(OwnedFd::from(reader))?;
        Ok::<_, std::io::Error>((child, rx))
    })();
    let (mut child, rx) = match spawned {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(fail(
                "The hub has no ssh program. Install OpenSSH on the hub, then try again."
                    .to_owned(),
                String::new(),
            ));
        }
        Err(e) => {
            return Err(fail(
                format!("The hub couldn't start ssh: {e}"),
                String::new(),
            ));
        }
    };

    let buffer = Arc::new(Mutex::new(Vec::new()));
    let mut reading = tokio::spawn(read_tail(rx, Arc::clone(&buffer)));
    let status = tokio::time::timeout(timeout, child.wait()).await;
    if status.is_err() {
        let _ = child.kill().await;
    }
    if tokio::time::timeout(GRACE, &mut reading).await.is_err() {
        reading.abort();
    }
    let bytes = std::mem::take(
        &mut *buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    // The code works once and is spent by now, but it still has no business on a page.
    let output = String::from_utf8_lossy(&bytes).replace(code, "<code>");

    let status = match status {
        Err(_) => {
            return Err(fail(
                format!(
                    "Gave up on {target} after {} minutes. Check it's on and on the tailnet, then try again.",
                    timeout.as_secs().div_ceil(60)
                ),
                tail(&output),
            ));
        }
        Ok(Err(e)) => {
            return Err(fail(
                format!("The hub lost track of ssh: {e}"),
                tail(&output),
            ));
        }
        Ok(Ok(s)) => s,
    };
    if status.success() {
        return Ok(Outcome { output });
    }
    Err(fail(explain(status.code(), &output, target), tail(&output)))
}

/// Reads everything written to the pipe, keeping the last `OUTPUT_CAP` bytes.
async fn read_tail(mut rx: tokio::net::unix::pipe::Receiver, buffer: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 8192];
    while let Ok(n) = rx.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        let mut b = buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        b.extend_from_slice(&chunk[..n]);
        if b.len() > OUTPUT_CAP {
            let extra = b.len() - OUTPUT_CAP;
            b.drain(..extra);
        }
    }
}

/// The last few lines of output.
fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// Plain words for a failed run. ssh itself exits 255; anything else is the install.
fn explain(code: Option<i32>, output: &str, target: &Target) -> String {
    let has = |words: &[&str]| words.iter().any(|w| output.contains(w));
    if code == Some(255) {
        if has(&[
            "Could not resolve hostname",
            "Name or service not known",
            "No route to host",
            "Connection timed out",
            "Connection refused",
            "Network is unreachable",
            "Operation timed out",
        ]) {
            return format!(
                "The hub couldn't reach {target}. Check the name and that the machine is on and on the tailnet, then try again."
            );
        }
        if has(&[
            "Host key verification failed",
            "REMOTE HOST IDENTIFICATION HAS CHANGED",
        ]) {
            return format!(
                "{target}'s SSH host key changed since the hub last signed in. If you expected that, remove its old line from ~/.ssh/known_hosts in the hub's account, then try again."
            );
        }
        return format!(
            "The hub couldn't sign in to {target} over SSH. Turn on Tailscale SSH on that machine (`sudo tailscale set --ssh`) or add the hub's SSH key to it, then try again."
        );
    }
    // The install script stops on a system it has no build for; say which, up front.
    if let Some(line) = output
        .lines()
        .map(str::trim)
        .find(|l| l.contains("isn't available for"))
    {
        if line.contains("for Darwin") {
            return format!("{target} runs macOS; atlasd is Linux-only for now.");
        }
        return format!("The install on {target} stopped: {line}");
    }
    format!("The install on {target} failed.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_plain_ssh_addresses_pass() {
        for ok in [
            "laptop",
            "me@laptop",
            "me@laptop.tailnet.ts.net",
            "_svc@10.0.0.2",
            " laptop ",
        ] {
            assert!(Target::parse(ok).is_ok(), "{ok:?} should pass");
        }
        for bad in [
            "-oProxyCommand=x",
            "a b",
            "me@-x",
            "$(x)",
            "a;b",
            "",
            "@host",
            "me@",
            "Me@laptop",
            "me@lap@top",
            "me@laptop:22",
            "`x`",
            "a\nb",
            "-x@laptop",
            &"a".repeat(254),
        ] {
            assert!(Target::parse(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(
            Target::parse("a;b")
                .unwrap_err()
                .contains("isn't an SSH address")
        );
    }

    /// A fake `ssh` that writes its arguments, one per line, then exits as told.
    fn fake_ssh(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("ssh");
        std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"$@\"\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const ORIGIN: &str = "https://hub.example.ts.net:7443";
    const CODE: &str = "0123abcd";

    async fn run_fake(body: &str, timeout: Duration) -> Result<Outcome, Failure> {
        let tmp = tempfile::tempdir().unwrap();
        let ssh = fake_ssh(tmp.path(), body);
        let target = Target::parse("you@laptop").unwrap();
        run(ssh.as_os_str(), ORIGIN, CODE, &target, timeout).await
    }

    #[tokio::test]
    async fn ssh_gets_the_exact_arguments() {
        let out = run_fake("exit 0", TIMEOUT).await.unwrap();
        let expected = [
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "--",
            "you@laptop",
            "curl -fsSL https://hub.example.ts.net:7443/install | sh -s -- <code>",
        ];
        assert_eq!(out.output.lines().collect::<Vec<_>>(), expected);
    }

    #[tokio::test]
    async fn failures_read_plainly() {
        let e = run_fake(
            "echo 'you@laptop: Permission denied (publickey).' >&2; exit 255",
            TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(
            e.message.contains("couldn't sign in to you@laptop"),
            "{}",
            e.message
        );
        assert!(e.message.contains("sudo tailscale set --ssh"));

        let e = run_fake(
            "echo 'ssh: Could not resolve hostname laptop: Name or service not known' >&2; exit 255",
            TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(
            e.message.contains("couldn't reach you@laptop"),
            "{}",
            e.message
        );

        let e = run_fake(
            "for i in $(seq 1 40); do echo line $i; done; exit 1",
            TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(e.message, "The install on you@laptop failed.");
        let lines: Vec<&str> = e.output.lines().collect();
        assert_eq!(lines.len(), TAIL_LINES);
        assert_eq!(lines.last(), Some(&"line 40"));
        assert!(!e.output.contains(CODE));
    }

    #[tokio::test]
    async fn an_unsupported_system_is_named_in_the_headline() {
        let e = run_fake(
            "echo \"atlasd isn't available for Darwin yet.\" >&2; exit 1",
            TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(
            e.message,
            "you@laptop runs macOS; atlasd is Linux-only for now."
        );
        assert!(e.output.contains("isn't available for Darwin"));

        let e = run_fake(
            "echo \"atlasd isn't available for riscv64 yet.\" >&2; exit 1",
            TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(
            e.message,
            "The install on you@laptop stopped: atlasd isn't available for riscv64 yet."
        );
    }

    #[tokio::test]
    async fn a_hung_install_is_stopped() {
        let start = std::time::Instant::now();
        let e = run_fake("echo started; sleep 30", Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(
            e.message.starts_with("Gave up on you@laptop"),
            "{}",
            e.message
        );
        assert!(e.output.contains("started"));
    }

    #[tokio::test]
    async fn a_missing_ssh_says_so() {
        let target = Target::parse("laptop").unwrap();
        let e = run(
            OsStr::new("/nonexistent/ssh"),
            ORIGIN,
            CODE,
            &target,
            TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(e.message.contains("no ssh program"));
    }
}
