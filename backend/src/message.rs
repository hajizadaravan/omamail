use crate::images;
use crate::mailbox::{Person, display_text, is_format_char, people};
use crate::sanitize::{clean_html, text_to_html};
use mail_parser::{Message, MessageParser, MessagePart, MimeHeaders, PartType};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct Attachment {
    pub index: usize,
    pub name: String,
    pub size: usize,
    pub mime: String,
}

#[derive(Serialize)]
pub struct View {
    pub uid: u32,
    pub subject: String,
    pub from: Vec<Person>,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    pub timestamp: i64,
    pub html: String,
    pub remote_images: usize,
    pub images_loaded: bool,
    pub attachments: Vec<Attachment>,
}

#[derive(Serialize)]
pub struct Draft {
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub in_reply_to: String,
    pub references: String,
    pub attachments: Vec<String>,
}

const MAX_HTML_BYTES: usize = 256 * 1024;

pub fn parse(raw: &[u8]) -> Option<Message<'_>> {
    MessageParser::default().parse(raw)
}

fn mime_of(part: &MessagePart) -> String {
    part.content_type()
        .map(|ct| match ct.subtype() {
            Some(sub) => format!("{}/{}", ct.ctype(), sub).to_ascii_lowercase(),
            None => ct.ctype().to_ascii_lowercase(),
        })
        .unwrap_or_else(|| "application/octet-stream".into())
}

/// Bytes of an attachment part; a forwarded mail is returned whole.
fn part_bytes<'a>(raw: &'a [u8], part: &'a MessagePart) -> &'a [u8] {
    match &part.body {
        PartType::Message(_) => raw
            .get(part.offset_body as usize..part.offset_end as usize)
            .unwrap_or_default(),
        _ => part.contents(),
    }
}

/// Last path segment only, with control/format characters and leading dots
/// removed, so a crafted name like "../../.bashrc" cannot escape the
/// download folder or disguise its extension.
pub fn safe_filename(name: &str, fallback: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base.chars().filter(|c| !c.is_control() && !is_format_char(*c)).collect();
    let cleaned = cleaned.trim_start_matches(|c: char| c == '.' || c.is_whitespace()).trim_end();
    let cleaned: String = cleaned.chars().take(180).collect();
    if cleaned.is_empty() { fallback.to_string() } else { cleaned }
}

fn attachment_name(part: &MessagePart, index: usize, mime: &str) -> String {
    let fallback = match &part.body {
        PartType::Message(m) => format!("{}.eml", m.subject().unwrap_or("message")),
        _ => {
            let ext = mime_guess::get_mime_extensions_str(mime).and_then(|e| e.first().copied()).unwrap_or("bin");
            format!("attachment-{}.{ext}", index + 1)
        }
    };
    safe_filename(part.attachment_name().unwrap_or(&fallback), &format!("attachment-{}", index + 1))
}

fn content_id(part: &MessagePart) -> Option<String> {
    part.content_id().map(|id| id.trim().trim_matches(['<', '>']).to_string())
}

pub fn render(raw: &[u8], uid: u32, load_images: bool) -> View {
    let Some(msg) = parse(raw) else {
        return View {
            uid,
            subject: String::new(),
            from: vec![],
            to: vec![],
            cc: vec![],
            timestamp: 0,
            html: text_to_html(&String::from_utf8_lossy(raw)),
            remote_images: 0,
            images_loaded: false,
            attachments: vec![],
        };
    };

    // html5ever's tree builder is quadratic in nesting depth: a few hundred
    // KB of crafted nesting would stall the backend for minutes. Gmail clips
    // mail at 102 KB, so anything far above that is shown as text instead.
    let html_part = msg.html_bodies().find(|p| p.is_text_html()).filter(|p| p.len() <= MAX_HTML_BYTES);
    let mut referenced: Vec<String> = Vec::new();
    let (html, remote_images, images_loaded) = match html_part.and_then(|p| p.text_contents()) {
        Some(source) => {
            let inline = extract_inline_images(&msg, source, &mut referenced);
            let first = clean_html(source, &inline, None);
            if load_images && !first.remote_images.is_empty() {
                let fetched = images::fetch_all(&first.remote_images);
                let second = clean_html(source, &inline, Some(&fetched));
                (second.html, first.remote_images.len(), true)
            } else {
                (first.html, first.remote_images.len(), false)
            }
        }
        None => {
            // body_text converts an HTML-only (or oversized HTML) body to text.
            let text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
            (text_to_html(&text), 0, false)
        }
    };

    let attachments = msg
        .attachments()
        .enumerate()
        .filter(|(_, part)| !content_id(part).is_some_and(|id| referenced.contains(&id)))
        .map(|(index, part)| {
            let mime = mime_of(part);
            Attachment {
                index,
                name: attachment_name(part, index, &mime),
                size: part_bytes(raw, part).len(),
                mime,
            }
        })
        .collect();

    View {
        uid,
        subject: display_text(msg.subject().unwrap_or_default()),
        from: people(msg.from()),
        to: people(msg.to()),
        cc: people(msg.cc()),
        timestamp: msg.date().map(|d| d.to_timestamp()).unwrap_or(0),
        html,
        remote_images,
        images_loaded,
        attachments,
    }
}

/// Writes `cid:`-referenced images to the cache so the reader can show them.
fn extract_inline_images(msg: &Message, html: &str, referenced: &mut Vec<String>) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    if !html.to_ascii_lowercase().contains("cid:") {
        return map;
    }
    let Ok(dir) = crate::config::private_cache("inline") else { return map };
    for part in msg.attachments() {
        let Some(id) = content_id(part) else { continue };
        let mime = mime_of(part);
        let ext = match mime.as_str() {
            "image/png" => "png",
            "image/jpeg" | "image/jpg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => continue,
        };
        let bytes = part.contents();
        let digest: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("{digest}.{ext}"));
        if !path.exists() && crate::config::write_private(&path, bytes).is_err() {
            continue;
        }
        referenced.push(id.clone());
        map.insert(id, path);
    }
    map
}

/// Plain text of the body (HTML converted), for reading codes out of it.
pub fn plain_text(raw: &[u8]) -> String {
    parse(raw).and_then(|m| m.body_text(0).map(|t| t.into_owned())).unwrap_or_default()
}

/// (file name, bytes) of the attachment at `index` in `render`'s numbering.
pub fn attachment(raw: &[u8], index: usize) -> Option<(String, Vec<u8>)> {
    let msg = parse(raw)?;
    let part = msg.attachments().nth(index)?;
    let mime = mime_of(part);
    Some((attachment_name(part, index, &mime), part_bytes(raw, part).to_vec()))
}

/// Every attachment, for forwarding.
pub fn all_attachments(raw: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    let Some(msg) = parse(raw) else { return vec![] };
    msg.attachments()
        .enumerate()
        .map(|(i, part)| {
            let mime = mime_of(part);
            (attachment_name(part, i, &mime), mime, part_bytes(raw, part).to_vec())
        })
        .collect()
}

/// Recipient list as editable text. Display names come from the sender, so
/// they are quoted: `Evil <a@x>, b@y` must stay one name, not two recipients.
fn format_people(list: &[Person]) -> String {
    list.iter()
        .map(|p| {
            let name: String = p.name.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').collect();
            if name.trim().is_empty() { p.email.clone() } else { format!("\"{name}\" <{}>", p.email) }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn prefixed(subject: &str, prefix: &str) -> String {
    let lower = subject.trim_start().to_ascii_lowercase();
    if lower.starts_with(&format!("{}:", prefix.to_ascii_lowercase())) {
        subject.to_string()
    } else {
        format!("{prefix}: {subject}")
    }
}

/// `mode`: "reply", "reply_all" or "forward". `me` is left out of recipients.
pub fn draft(raw: &[u8], mode: &str, me: &str) -> Option<Draft> {
    let msg = parse(raw)?;
    let from = people(msg.from());
    let reply_to = people(msg.reply_to());
    let to = people(msg.to());
    let cc = people(msg.cc());
    let subject = msg.subject().unwrap_or_default();
    let date = msg.date().map(|d| d.to_rfc822()).unwrap_or_default();
    let text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    let not_me = |p: &&Person| !p.email.eq_ignore_ascii_case(me) && !p.email.is_empty();

    let message_id = msg.message_id().map(|id| format!("<{id}>")).unwrap_or_default();
    let mut references: Vec<String> = msg
        .references()
        .as_text_list()
        .map(|l| l.iter().map(|r| format!("<{r}>")).collect())
        .or_else(|| msg.references().as_text().map(|r| vec![format!("<{r}>")]))
        .or_else(|| msg.in_reply_to().as_text().map(|r| vec![format!("<{r}>")]))
        .unwrap_or_default();
    if !message_id.is_empty() {
        references.push(message_id.clone());
    }

    if mode == "forward" {
        let header = format!(
            "\n\n---------- Forwarded message ----------\nFrom: {}\nDate: {}\nSubject: {}\nTo: {}\n\n",
            format_people(&from),
            date,
            subject,
            format_people(&to)
        );
        return Some(Draft {
            to: String::new(),
            cc: String::new(),
            subject: prefixed(subject, "Fwd"),
            body: header + &text,
            in_reply_to: String::new(),
            references: String::new(),
            attachments: all_attachments(raw).into_iter().map(|(name, _, _)| name).collect(),
        });
    }

    let mut primary = if reply_to.is_empty() { from.clone() } else { reply_to };
    // Replying to your own sent mail goes to its recipients, not to you.
    if !primary.is_empty() && primary.iter().all(|p| p.email.eq_ignore_ascii_case(me)) {
        let others: Vec<Person> = to.iter().filter(not_me).cloned().collect();
        // A note to self has no one else to answer.
        if !others.is_empty() {
            primary = others;
        }
    }
    let (to_list, cc_list): (Vec<Person>, Vec<Person>) = if mode == "reply_all" {
        let mut to_all: Vec<Person> = Vec::new();
        for p in primary.iter().chain(to.iter()).filter(not_me) {
            if !to_all.iter().any(|t| t.email.eq_ignore_ascii_case(&p.email)) {
                to_all.push(p.clone());
            }
        }
        let cc_all: Vec<Person> = cc
            .iter()
            .filter(not_me)
            .filter(|p| !to_all.iter().any(|t| t.email.eq_ignore_ascii_case(&p.email)))
            .cloned()
            .collect();
        (to_all, cc_all)
    } else {
        (primary, vec![])
    };

    let who = from.first().map(|p| if p.name.is_empty() { p.email.clone() } else { p.name.clone() }).unwrap_or_default();
    let quoted: String = text.lines().map(|l| if l.starts_with('>') { format!(">{l}\n") } else { format!("> {l}\n") }).collect();
    Some(Draft {
        to: format_people(&to_list),
        cc: format_people(&cc_list),
        subject: prefixed(subject, "Re"),
        body: format!("\n\nOn {date}, {who} wrote:\n{quoted}"),
        in_reply_to: message_id,
        references: references.join(" "),
        attachments: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIL: &[u8] = b"From: Ali <ali@example.az>\r\n\
To: me@gmail.com, Leyla <leyla@example.az>\r\n\
Cc: Me Again <ME@gmail.com>, Rauf <rauf@example.az>\r\n\
Subject: =?UTF-8?B?R8O2csO8xZ8=?=\r\n\
Message-ID: <abc@example.az>\r\n\
References: <root@example.az>\r\n\
Date: Fri, 16 Oct 2026 10:00:00 +0400\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=b\r\n\
\r\n\
--b\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Salam <img src=\"https://t.example/p.gif\"></p><script>x</script>\r\n\
--b\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment; filename=\"../../evil/.bashrc\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0=\r\n\
--b--\r\n";

    #[test]
    fn renders_sanitized_view() {
        let view = render(MAIL, 7, false);
        assert_eq!(view.subject, "Görüş");
        assert_eq!(view.from[0].email, "ali@example.az");
        assert!(view.html.contains("Salam") && !view.html.contains("script"), "{}", view.html);
        assert_eq!(view.remote_images, 1);
        assert_eq!(view.attachments.len(), 1);
        assert_eq!(view.attachments[0].name, "bashrc");
        assert_eq!(attachment(MAIL, 0).unwrap().1, b"%PDF-");
    }

    #[test]
    fn reply_all_skips_me_and_threads() {
        let d = draft(MAIL, "reply_all", "me@gmail.com").unwrap();
        assert_eq!(d.to, "\"Ali\" <ali@example.az>, \"Leyla\" <leyla@example.az>");
        assert_eq!(d.cc, "\"Rauf\" <rauf@example.az>");
        assert_eq!(d.subject, "Re: Görüş");
        assert_eq!(d.in_reply_to, "<abc@example.az>");
        assert_eq!(d.references, "<root@example.az> <abc@example.az>");
    }

    #[test]
    fn reply_to_own_mail_goes_to_its_recipients() {
        let mail = b"From: Me <me@gmail.com>\r\nTo: Ali <ali@x.az>, Me <me@gmail.com>\r\nCc: Ali <ALI@x.az>\r\nSubject: s\r\nIn-Reply-To: <p@x.az>\r\n\r\nbody\r\n";
        let d = draft(mail, "reply_all", "me@gmail.com").unwrap();
        assert_eq!(d.to, "\"Ali\" <ali@x.az>");
        assert_eq!(d.cc, "");
        assert_eq!(d.references, "<p@x.az>");
    }

    #[test]
    fn crafted_display_names_stay_one_recipient() {
        let list = vec![
            Person { name: "Evil <x@a.az>, y@b.az".into(), email: "real@z.az".into() },
            Person { name: "Q\"uote".into(), email: "q@z.az".into() },
            Person { name: "Rauf M. Məmmədov".into(), email: "r@z.az".into() },
        ];
        let text = format_people(&list);
        let parsed = crate::send::parse_recipients(&text).unwrap();
        let emails: Vec<String> = parsed.iter().map(|m| m.email.to_string()).collect();
        assert_eq!(emails, vec!["real@z.az", "q@z.az", "r@z.az"], "{text}");
    }

    #[test]
    fn forward_keeps_attachments() {
        let d = draft(MAIL, "forward", "me@gmail.com").unwrap();
        assert_eq!(d.subject, "Fwd: Görüş");
        assert!(d.to.is_empty() && d.in_reply_to.is_empty());
        assert_eq!(d.attachments, vec!["bashrc"]);
    }

    #[test]
    fn oversized_html_is_shown_as_text_quickly() {
        let depth = 60_000;
        let body = "<div>x".repeat(depth) + &"</div>".repeat(depth);
        let mail = format!("Subject: big\r\nContent-Type: text/html\r\n\r\n<p>Hello <b>there</b></p>{body}\r\n");
        let started = std::time::Instant::now();
        let view = render(mail.as_bytes(), 1, false);
        assert!(started.elapsed().as_secs() < 2, "took {:?}", started.elapsed());
        assert!(view.html.contains("Hello") && !view.html.contains("<b>"), "{}", &view.html[..200]);
    }

    #[test]
    fn filenames_cannot_escape() {
        assert_eq!(safe_filename("../../x/.ssh", "f"), "ssh");
        assert_eq!(safe_filename("a\\b\\c.txt", "f"), "c.txt");
        assert_eq!(safe_filename("..", "f"), "f");
        assert_eq!(safe_filename("ok\u{0}name.pdf", "f"), "okname.pdf");
        assert_eq!(safe_filename("invoice\u{202E}fdp.exe", "f"), "invoicefdp.exe");
        // Dots and spaces in turn: no pass may leave a hidden name behind.
        assert_eq!(safe_filename(". .bashrc", "f"), "bashrc");
        assert_eq!(safe_filename(" . . .profile ", "f"), "profile");
        assert_eq!(safe_filename(". .", "f"), "f");
        assert_eq!(safe_filename(".\u{3000}.config", "f"), "config");
        assert_eq!(prefixed("RE: x", "Re"), "RE: x");
    }
}
