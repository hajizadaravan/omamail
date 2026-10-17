//! `omamail watch`: the user service behind new-mail notifications.
//!
//! Sits in IMAP IDLE on INBOX and posts one desktop notification per new
//! unread message (one summary when several land at once). Clicking it opens
//! the message in the Omamail window. Only mail that arrives while the
//! watcher runs is announced: the first start records the current UIDNEXT
//! instead of replaying the whole inbox.

use crate::config::{self, state_dir};
use crate::mailbox::{MailError, Mailbox, Summary};
use crate::secret;
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use std::time::Duration;

const PLUGIN_ID: &str = "reavann.omamail";
const INBOX: &str = "INBOX";
const GLYPH: &str = "\u{f01ee}"; // nf-md-email

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// The account the UIDs belong to; a different account starts fresh.
    #[serde(default)]
    email: String,
    uidvalidity: u32,
    last_uid: u32,
}

fn state_file() -> std::path::PathBuf {
    state_dir().join("watch.json")
}

fn load_state() -> State {
    std::fs::read_to_string(state_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_state(state: &State) {
    if let Ok(text) = serde_json::to_string(state) {
        let _ = config::write_private(&state_file(), text.as_bytes());
    }
}

/// Notification bodies are rendered as markup by the Omarchy shell.
fn markup_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn sender(s: &Summary) -> String {
    s.from
        .first()
        .map(|p| if p.name.is_empty() { p.email.clone() } else { p.name.clone() })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Unknown sender".into())
}

fn notify(summary: String, body: String, open_uid: Option<u32>) {
    std::thread::spawn(move || {
        let output = Command::new("notify-send")
            .args(["-a", "Omamail", "-h", &format!("string:omarchy-glyph:{GLYPH}")])
            .args(["--action=default=Open", "--wait", "--", &summary, &body])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let clicked = output.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "default");
        if clicked {
            let payload = match open_uid {
                Some(uid) => format!(r#"{{"folder":"{INBOX}","uid":{uid}}}"#),
                None => format!(r#"{{"folder":"{INBOX}"}}"#),
            };
            let _ = Command::new("omarchy-shell")
                .args(["shell", "summon", PLUGIN_ID, &payload])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    });
}

fn announce(new: &[Summary]) {
    let unread: Vec<&Summary> = new.iter().filter(|s| !s.seen).collect();
    match unread.as_slice() {
        [] => {}
        [one] => notify(sender(one), markup_escape(&one.subject), Some(one.uid)),
        many => {
            let lines: Vec<String> =
                many.iter().take(3).map(|s| markup_escape(&format!("{}: {}", sender(s), s.subject))).collect();
            notify(format!("{} new messages", many.len()), lines.join("\n"), None);
        }
    }
}

fn check(mailbox: &mut Mailbox, state: &mut State, email: &str) -> Result<(), MailError> {
    let (validity, next) = mailbox.uid_status(INBOX)?;
    if validity != state.uidvalidity || state.last_uid == 0 || state.email != email {
        // First run, or the server renumbered the folder: start from now.
        state.email = email.to_string();
        state.uidvalidity = validity;
        state.last_uid = next.saturating_sub(1);
        save_state(state);
        return Ok(());
    }
    if next.saturating_sub(1) <= state.last_uid {
        return Ok(());
    }
    let new = mailbox.new_since(INBOX, state.last_uid)?;
    if let Some(max) = new.iter().map(|s| s.uid).max() {
        state.last_uid = max;
        save_state(state);
        announce(&new);
    }
    Ok(())
}

pub fn run() -> i32 {
    let Some(account) = config::load_account() else {
        eprintln!("omamail watch: no account configured; sign in from the Omamail window first");
        return 2;
    };
    let Some(password) = secret::lookup(&account.email) else {
        eprintln!("omamail watch: no password in the keyring for {}", account.email);
        return 2;
    };
    let email = account.email.clone();
    let mut mailbox = Mailbox::new(account, password);
    let mut state = load_state();
    let mut backoff = 5;
    loop {
        let result = check(&mut mailbox, &mut state, &email)
            .and_then(|_| mailbox.wait_for_change(INBOX, Duration::from_secs(9 * 60)).map(|_| ()));
        match result {
            Ok(()) => backoff = 5,
            Err(MailError::Auth) => {
                eprintln!("omamail watch: Gmail rejected the stored app password");
                return 3;
            }
            Err(e) => {
                eprintln!("omamail watch: {e}; retrying in {backoff}s");
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(300);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup_in_subjects() {
        assert_eq!(markup_escape("<b>Sale</b> & more"), "&lt;b&gt;Sale&lt;/b&gt; &amp; more");
    }
}
