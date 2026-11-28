//! Installing atlasd on another machine over SSH from the owner's workstation.
//!
//! Unlike the hub's headless SSH runner, `atlasd machines add` runs in the owner's own
//! terminal, so it inherits stdio and lets SSH use the workstation's keys, agent, or
//! interactive prompts.

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// An SSH address checked to be safe as an `ssh` argument: `host` or `user@host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target(String);

impl Target {
    /// Checks an SSH target typed by the owner.
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
            return Err("Give the machine's SSH address, such as you@laptop.".to_owned());
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

fn shell_safe(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ":/._-[]".contains(c))
}

fn ssh_args(origin: &str, code: &str, target: &Target) -> Vec<String> {
    vec![
        "-o".to_owned(),
        "ConnectTimeout=10".to_owned(),
        "-o".to_owned(),
        "StrictHostKeyChecking=accept-new".to_owned(),
        "--".to_owned(),
        target.as_str().to_owned(),
        format!("curl -fsSL {origin}/install | sh -s -- {code}"),
    ]
}

/// Runs `ssh` from this machine to install and pair `atlasd` on `target`.
///
/// # Errors
/// If the origin/code cannot be passed safely, `ssh` can't start, or the remote install fails.
pub fn install_over_ssh(origin: &str, code: &str, target: &Target) -> Result<(), String> {
    run_with(OsStr::new("ssh"), origin, code, target)
}

fn run_with(program: &OsStr, origin: &str, code: &str, target: &Target) -> Result<(), String> {
    if !shell_safe(origin) || !shell_safe(code) {
        return Err(
            "the hub's address or pairing code has characters a command can't carry".to_owned(),
        );
    }
    let status = Command::new(program)
        .args(ssh_args(origin, code, target))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "ssh isn't installed on this machine".to_owned()
            } else {
                format!("couldn't start ssh: {e}")
            }
        })?;
    if status.success() {
        Ok(())
    } else if status.code() == Some(255) {
        Err(format!(
            "ssh to {target} failed (exit 255). Check that you can run `ssh {target}`."
        ))
    } else {
        Err(format!("the install on {target} failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn validates_ssh_targets() {
        for ok in [
            "laptop",
            "me@laptop",
            "me@laptop.tailnet.ts.net",
            "10.0.0.2",
        ] {
            assert!(Target::parse(ok).is_ok(), "{ok}");
        }
        for bad in ["-oProxyCommand=x", "a;b", "a b", "me@", "@host", "me@h:22"] {
            assert!(Target::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn passes_exact_arguments_to_ssh() {
        let tmp = tempfile::tempdir().unwrap();
        let out_file = tmp.path().join("args.txt");
        let fake = tmp.path().join("ssh");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
                out_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let target = Target::parse("me@laptop").unwrap();
        run_with(
            fake.as_os_str(),
            "https://hub.example.ts.net:7443",
            "abc123",
            &target,
        )
        .unwrap();
        let recorded = std::fs::read_to_string(&out_file).unwrap();
        let lines: Vec<&str> = recorded.lines().collect();
        assert_eq!(
            lines,
            vec![
                "-o",
                "ConnectTimeout=10",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "--",
                "me@laptop",
                "curl -fsSL https://hub.example.ts.net:7443/install | sh -s -- abc123",
            ]
        );
    }
}
