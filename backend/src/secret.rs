//! App password storage in the desktop keyring (libsecret via `secret-tool`).
//!
//! The password only ever travels over pipes, never argv, so it never shows
//! up in /proc/<pid>/cmdline.

use std::io::Write;
use std::process::{Command, Stdio};

const ATTRS: [&str; 3] = ["application", "omamail", "account"];

pub fn store(account: &str, password: &str) -> Result<(), String> {
    let mut child = Command::new("secret-tool")
        .arg("store")
        .arg(format!("--label=Omamail ({account})"))
        .args(ATTRS)
        .arg(account)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("secret-tool is not available: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(password.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not save the password to the keyring: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

pub fn lookup(account: &str) -> Option<String> {
    let out = Command::new("secret-tool")
        .arg("lookup")
        .args(ATTRS)
        .arg(account)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let password = String::from_utf8(out.stdout).ok()?;
    (out.status.success() && !password.is_empty()).then_some(password)
}

pub fn clear(account: &str) {
    let _ = Command::new("secret-tool")
        .arg("clear")
        .args(ATTRS)
        .arg(account)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
