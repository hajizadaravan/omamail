use crate::config::Account;
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

/// Gmail rejects messages over 25 MB; base64 adds about a third.
const MAX_ATTACHMENT_BYTES: u64 = 18 * 1024 * 1024;

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Outgoing {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    pub files: Vec<String>,
    pub in_reply_to: String,
    pub references: String,
    /// Forward the original mail's attachments: (folder, uid) of the source.
    pub forward_folder: String,
    pub forward_uid: u32,
}

/// `<random@sender-domain>`: lettre's default would put this machine's
/// hostname into every sent mail.
fn message_id(email: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let seed = format!("{nanos}-{}-{}-{email}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed));
    let id: String = Sha256::digest(seed.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect();
    let domain = email.rsplit('@').next().filter(|d| !d.is_empty()).unwrap_or("omamail.invalid");
    format!("<{id}@{domain}>")
}

pub fn parse_recipients(field: &str) -> Result<Vec<Mailbox>, String> {
    // Split on commas outside quotes so `"Doe, John" <j@x.az>` stays whole.
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in field.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                current.push(ch);
            }
            ',' | ';' if !quoted => parts.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<Mailbox>().map_err(|_| format!("Not a valid address: {p}")))
        .collect()
}

fn single_line(value: &str, field: &str) -> Result<String, String> {
    if value.contains(['\r', '\n']) {
        return Err(format!("{field} cannot contain line breaks"));
    }
    Ok(value.trim().to_string())
}

pub fn send(
    account: &Account,
    password: &str,
    out: &Outgoing,
    forwarded: Vec<(String, String, Vec<u8>)>,
) -> Result<(), String> {
    let to = parse_recipients(&out.to)?;
    let cc = parse_recipients(&out.cc)?;
    let bcc = parse_recipients(&out.bcc)?;
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err("Add at least one recipient".into());
    }
    let from: Mailbox = account.email.parse().map_err(|_| "Your account address is invalid".to_string())?;

    // lettre only generates a Message-ID when asked; replies need one.
    let mut builder = Message::builder()
        .from(from)
        .subject(single_line(&out.subject, "Subject")?)
        .date_now()
        .message_id(Some(message_id(&account.email)));
    for m in to {
        builder = builder.to(m);
    }
    for m in cc {
        builder = builder.cc(m);
    }
    for m in bcc {
        builder = builder.bcc(m);
    }
    let in_reply_to = single_line(&out.in_reply_to, "In-Reply-To")?;
    if !in_reply_to.is_empty() {
        builder = builder.in_reply_to(in_reply_to);
    }
    let references = single_line(&out.references, "References")?;
    if !references.is_empty() {
        builder = builder.references(references);
    }

    let mut attachments: Vec<SinglePart> = Vec::new();
    let mut total: u64 = 0;
    for (name, mime, bytes) in forwarded {
        total += bytes.len() as u64;
        let ct = ContentType::parse(&mime).unwrap_or(ContentType::parse("application/octet-stream").unwrap());
        attachments.push(Attachment::new(name).body(bytes, ct));
    }
    for file in &out.files {
        let path = Path::new(file);
        let meta = std::fs::metadata(path).map_err(|e| format!("{file}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{file} is not a file"));
        }
        total += meta.len();
        if total > MAX_ATTACHMENT_BYTES {
            return Err("Attachments are too large for Gmail (about 18 MB in total)".into());
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{file}: {e}"))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let ct = ContentType::parse(mime.as_ref()).unwrap_or(ContentType::parse("application/octet-stream").unwrap());
        attachments.push(Attachment::new(name).body(bytes, ct));
    }
    if total > MAX_ATTACHMENT_BYTES {
        return Err("Attachments are too large for Gmail (about 18 MB in total)".into());
    }

    let text = SinglePart::plain(out.body.clone());
    let message = if attachments.is_empty() {
        builder.singlepart(text)
    } else {
        let mut mixed = MultiPart::mixed().singlepart(text);
        for a in attachments {
            mixed = mixed.singlepart(a);
        }
        builder.multipart(mixed)
    }
    .map_err(|e| format!("Could not build the message: {e}"))?;

    // Gmail stores a copy in Sent by itself, so nothing is appended over IMAP.
    let transport = SmtpTransport::relay(&account.smtp_host)
        .map_err(|e| e.to_string())?
        .port(account.smtp_port)
        .credentials(Credentials::new(account.email.clone(), password.to_string()))
        .timeout(Some(Duration::from_secs(60)))
        .build();
    transport.send(&message).map_err(|e| {
        if e.is_permanent() && e.to_string().contains("535") {
            "Gmail rejected the app password".to_string()
        } else {
            format!("Sending failed: {e}")
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_recipients_outside_quotes() {
        let list = parse_recipients(r#""Doe, John" <j@x.az>; a@b.az ,"#).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].email.to_string(), "j@x.az");
        assert_eq!(list[1].email.to_string(), "a@b.az");
        assert!(parse_recipients("not an address").is_err());
        assert!(parse_recipients("a@b.az\r\nBcc: x@y.az").is_err());
    }

    #[test]
    fn message_ids_are_unique_and_hide_the_hostname() {
        let a = message_id("me@gmail.com");
        let b = message_id("me@gmail.com");
        assert_ne!(a, b);
        assert!(a.starts_with('<') && a.ends_with("@gmail.com>"), "{a}");
    }

    #[test]
    fn header_fields_reject_line_breaks() {
        assert!(single_line("Hi\r\nBcc: x@y.az", "Subject").is_err());
        assert_eq!(single_line(" ok ", "Subject").unwrap(), "ok");
    }
}
