//! Untrusted mail HTML → the small rich-text subset Qt's TextEdit renders.
//!
//! Everything that could act on the reader is dropped: scripts, styles,
//! forms, event handlers, and every attribute not on the allowlist. Inline
//! styles go too, so mail follows the Omarchy theme instead of painting its
//! own background. Remote images are blocked unless the caller passes a map
//! of already-downloaded copies, so opening a mail never pings a tracker.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_IMAGE_WIDTH: u32 = 560;

const TAGS: &[&str] = &[
    "a", "abbr", "b", "big", "blockquote", "br", "caption", "center", "cite", "code", "dd", "del", "div", "dl",
    "dt", "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i", "img", "ins", "kbd", "li", "ol", "p", "pre", "q",
    "s", "samp", "small", "span", "strike", "strong", "sub", "sup", "table", "tbody", "td", "tfoot", "th",
    "thead", "tr", "tt", "u", "ul", "var",
];
const DROP_WITH_CONTENT: &[&str] = &["script", "style", "title", "noscript", "template", "head", "svg", "math"];
/// Marker the attribute filter leaves on elements the sender hid with CSS
/// (newsletter preheaders, tracking blocks). Every other style is dropped, so
/// once styles are gone the hidden text would otherwise show up.
const HIDDEN: &str = " style=\"display:none\"";

pub struct Clean {
    pub html: String,
    /// Remote image URLs found, in document order, whether or not they were shown.
    pub remote_images: Vec<String>,
}

/// `inline`: `cid:` reference (without the scheme) → file of the extracted part.
/// `remote`: remote URL → downloaded copy; `None` blocks all remote images.
pub fn clean_html(
    html: &str,
    inline: &HashMap<String, PathBuf>,
    remote: Option<&HashMap<String, PathBuf>>,
) -> Clean {
    let found: Arc<Mutex<Vec<String>>> = Arc::default();
    let inline = Arc::new(inline.clone());
    let remote = Arc::new(remote.cloned());
    let filter_found = Arc::clone(&found);

    let mut tag_attrs: HashMap<&str, HashSet<&str>> = HashMap::new();
    // Only attributes whose values the post-processing can trust: nothing
    // free-text (title, alt) that could carry tag-like characters.
    tag_attrs.insert("a", ["href"].into());
    tag_attrs.insert("img", ["src", "width"].into());
    tag_attrs.insert("td", ["colspan", "rowspan"].into());
    tag_attrs.insert("th", ["colspan", "rowspan"].into());
    tag_attrs.insert("ol", ["start"].into());

    let mut builder = ammonia::Builder::empty();
    builder
        .tags(TAGS.iter().copied().collect())
        .clean_content_tags(DROP_WITH_CONTENT.iter().copied().collect())
        .tag_attributes(tag_attrs)
        .url_schemes(["http", "https", "mailto", "cid"].into())
        .generic_attributes(["style"].into())
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(move |tag, attr, value| -> Option<Cow<'_, str>> {
            match (tag, attr) {
                ("a", "href") => is_link(value).then_some(Cow::Borrowed(value)),
                ("img", "src") => {
                    if let Some(cid) = strip_scheme(value, "cid:") {
                        return inline.get(cid.trim_matches(['<', '>'])).map(|p| file_url(p).into());
                    }
                    if is_web(value) {
                        if let Ok(mut list) = filter_found.lock()
                            && !list.iter().any(|u| u == value)
                        {
                            list.push(value.to_string());
                        }
                        return remote.as_ref().as_ref().and_then(|m| m.get(value)).map(|p| file_url(p).into());
                    }
                    None
                }
                (_, "style") => is_hidden_style(value).then_some(Cow::Borrowed("display:none")),
                (_, "width" | "colspan" | "rowspan" | "start") => {
                    let digits = value.trim().trim_end_matches("px");
                    digits.parse::<u32>().ok().filter(|n| *n <= 4000).map(|n| n.to_string().into())
                }
                _ => Some(Cow::Borrowed(value)),
            }
        });
    let cleaned = builder.clean(html).to_string();
    let remote_images = found.lock().map(|l| l.clone()).unwrap_or_default();
    Clean { html: tidy(&fit_images(&remove_hidden(&cleaned))), remote_images }
}

/// Newsletters are built from dozens of nested layout tables, and Qt adds
/// spacing and padding to every one, so text drifts right in steps and
/// blocks float apart. Mail reads better as a plain flow: rows become
/// blocks, cells become inline runs, and the empty shells left behind by
/// blocked images and spacer cells are dropped.
fn tidy(html: &str) -> String {
    let html = html.replace(GAP, "");
    collapse_spaces(&trim_links(&place_gaps(&flatten(&html))))
}

/// Marks the end of a table, i.e. of a visual card in the original layout.
const GAP: char = '\u{1}';

const EMPTYABLE: &[&str] = &[
    "a", "b", "big", "blockquote", "center", "div", "em", "h3", "h4", "i", "li", "ol", "p", "small", "span",
    "strong", "u", "ul",
];

/// Output name for tags that flatten() rewrites; None inside means the
/// tag disappears (table wrappers).
fn mapped(name: &str) -> Option<Option<&'static str>> {
    Some(match name {
        "table" | "tbody" | "thead" | "tfoot" => None,
        "tr" => Some("div"),
        "td" => Some("span"),
        "th" => Some("b"),
        // Qt sizes headings by level and ignores CSS for them, and mail h1s
        // are poster-sized; two smaller levels read calmer.
        "h1" | "h2" => Some("h3"),
        "h3" | "h4" | "h5" | "h6" => Some("h4"),
        _ => return None,
    })
}

struct Open {
    source: String,
    /// Element name in the output, None for vanished table wrappers.
    name: Option<String>,
    start: usize,
    content: bool,
    nbsp: bool,
}

/// One pass with a stack, linear in the input: flattens tables (marking each
/// table end with GAP) and drops elements left with nothing visible inside.
/// A `<p>&nbsp;</p>` outside the tables is a deliberate blank line (Outlook
/// writes them), so it becomes `<br>` instead.
fn flatten(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut stack: Vec<Open> = Vec::new();
    let mut cells = 0usize;
    let mut pre = 0usize;
    let mut rest = html;
    while !rest.is_empty() {
        let lt = rest.find('<').unwrap_or(rest.len());
        if lt > 0 {
            note_text(&rest[..lt], pre > 0, &mut stack);
            out.push_str(&rest[..lt]);
            rest = &rest[lt..];
            continue;
        }
        let Some(gt) = rest.find('>').map(|i| i + 1) else {
            out.push_str(rest);
            break;
        };
        let tag = &rest[..gt];
        rest = &rest[gt..];
        let closing = tag.starts_with("</");
        let source: String =
            tag.trim_start_matches(['<', '/']).chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if matches!(source.as_str(), "br" | "hr" | "img") {
            if let Some(top) = stack.last_mut() {
                top.content = true;
            }
            out.push_str(tag);
            continue;
        }
        if !closing {
            let start = out.len();
            let name = match mapped(&source) {
                Some(Some(to)) => {
                    out.push_str(&format!("<{to}>"));
                    Some(to.to_string())
                }
                Some(None) => None,
                None => {
                    out.push_str(tag);
                    Some(source.clone())
                }
            };
            if matches!(source.as_str(), "td" | "th") {
                cells += 1;
            }
            if source == "pre" {
                pre += 1;
            }
            stack.push(Open { source, name, start, content: false, nbsp: false });
            continue;
        }
        // ammonia's output is balanced; anything unexpected passes through.
        if stack.last().is_none_or(|o| o.source != source) {
            out.push_str(tag);
            continue;
        }
        let open = stack.pop().expect("checked above");
        let cell = matches!(source.as_str(), "td" | "th");
        if cell {
            cells -= 1;
        }
        if source == "pre" {
            pre -= 1;
        }
        let kept = match &open.name {
            None => {
                if open.content && source == "table" {
                    out.push(GAP);
                }
                open.content
            }
            Some(name) if open.content || !EMPTYABLE.contains(&name.as_str()) => {
                out.push_str(&format!("</{name}>"));
                if cell {
                    out.push(' ');
                }
                true
            }
            Some(_) if open.nbsp && cells == 0 && matches!(source.as_str(), "p" | "div") => {
                out.truncate(open.start);
                out.push_str("<br>");
                true
            }
            Some(_) => {
                out.truncate(open.start);
                out.push(' ');
                // `<p><span>&nbsp;</span></p>`: the blank line belongs to the p.
                if open.nbsp && !cell && let Some(parent) = stack.last_mut() {
                    parent.nbsp = true;
                }
                false
            }
        };
        if kept && let Some(parent) = stack.last_mut() {
            parent.content = true;
        }
    }
    out
}

/// Inside `<pre>` whitespace is content (indentation in highlighted code).
fn note_text(text: &str, in_pre: bool, stack: &mut [Open]) {
    let Some(top) = stack.last_mut() else { return };
    let visible = text.replace("&nbsp;", "").chars().any(|c| !c.is_whitespace() && c != '\u{a0}');
    if visible || (in_pre && !text.is_empty()) {
        top.content = true;
    } else if text.contains("&nbsp;") {
        top.nbsp = true;
    }
}

/// True when byte `i` falls inside an `&nbsp;` entity.
fn in_nbsp(text: &str, i: usize) -> bool {
    (i.saturating_sub(5)..=i).any(|s| text.get(s..).is_some_and(|t| t.starts_with("&nbsp;")))
}

/// One blank line per run of card ends that has visible content on both
/// sides; ends with nothing visible in between (nested tables closing
/// together, spacer tables) or at the very start or end produce nothing.
fn place_gaps(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut seen_content = false;
    let mut rest = html;
    while let Some(c) = rest.chars().next() {
        match c {
            GAP => {
                let after = &rest[c.len_utf8()..];
                if seen_content && has_content_before_next_gap(after) {
                    out.push_str("<br>");
                }
                rest = after;
            }
            '<' => {
                let end = rest.find('>').map(|e| e + 1).unwrap_or(rest.len());
                if rest.starts_with("<img") {
                    seen_content = true;
                }
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            }
            '&' if rest.starts_with("&nbsp;") => {
                out.push_str("&nbsp;");
                rest = &rest[6..];
            }
            _ => {
                if !c.is_whitespace() {
                    seen_content = true;
                }
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

fn has_content_before_next_gap(rest: &str) -> bool {
    let mut in_tag = false;
    let mut tag_start = 0;
    for (i, c) in rest.char_indices() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                if rest[tag_start..i].starts_with("<img") {
                    return true;
                }
            }
            continue;
        }
        match c {
            GAP => return false,
            '<' => {
                in_tag = true;
                tag_start = i;
            }
            c if c.is_whitespace() || in_nbsp(rest, i) => {}
            _ => return true,
        }
    }
    false
}

/// Runs of ASCII whitespace in text collapse to one space; they only made
/// the serialized output large. Tags and `<pre>` blocks are left as they are.
fn collapse_spaces(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut pre = 0usize;
    let mut last_space = false;
    let mut rest = html;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            let end = rest.find('>').map(|i| i + 1).unwrap_or(rest.len());
            let tag = &rest[..end];
            if tag.starts_with("<pre") && matches!(tag.as_bytes().get(4), Some(b'>') | Some(b' ')) {
                pre += 1;
            } else if tag == "</pre>" {
                pre = pre.saturating_sub(1);
            }
            out.push_str(tag);
            rest = &rest[end..];
            last_space = false;
            continue;
        }
        if pre == 0 && c.is_ascii_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `<a href="x"> Apply now </a>` → ` <a href="x">Apply now</a> `, so link
/// styling does not extend over the padding spaces.
fn trim_links(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<a ") {
        let Some(open_end) = rest[start..].find('>').map(|i| start + i + 1) else { break };
        let Some(close) = rest[open_end..].find("</a>").map(|i| open_end + i) else { break };
        let inner = &rest[open_end..close];
        out.push_str(&rest[..start]);
        if inner.contains('<') {
            out.push_str(&rest[start..close + 4]);
        } else {
            if inner.starts_with(char::is_whitespace) {
                out.push(' ');
            }
            out.push_str(&rest[start..open_end]);
            out.push_str(inner.trim());
            out.push_str("</a>");
            if inner.ends_with(char::is_whitespace) {
                out.push(' ');
            }
        }
        rest = &rest[close + 4..];
    }
    out.push_str(rest);
    out
}

fn is_hidden_style(style: &str) -> bool {
    let compact: String = style.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    let zero = |prop: &str| {
        compact.split(';').any(|decl| {
            decl.strip_prefix(prop)
                .and_then(|v| v.strip_prefix(':'))
                .is_some_and(|v| matches!(v.trim_end_matches("!important"), "0" | "0px" | "0%"))
        })
    };
    compact.contains("display:none")
        || compact.contains("visibility:hidden")
        || compact.contains("mso-hide:all")
        || zero("max-height")
        || zero("opacity")
}

/// Removes every element carrying the hidden marker, with its content.
/// ammonia's output is serialized from a DOM, so tags are balanced and the
/// marker can only come from the attribute filter (text is escaped).
fn remove_hidden(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(marker) = rest.find(HIDDEN) {
        // Text nodes keep their quotes, so the marker can appear in plain
        // text; only a marker inside an open tag counts.
        let open = rest[..marker].rfind('<');
        let Some(open) = open.filter(|o| !rest[*o..marker].contains('>')) else {
            out.push_str(&rest[..marker + HIDDEN.len()]);
            rest = &rest[marker + HIDDEN.len()..];
            continue;
        };
        let name: String = rest[open + 1..marker].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        out.push_str(&rest[..open]);
        let Some(tag_end) = rest[marker..].find('>').map(|i| marker + i + 1) else {
            rest = "";
            break;
        };
        if matches!(name.as_str(), "br" | "hr" | "img") {
            rest = &rest[tag_end..];
            continue;
        }
        // Walk the following tags once, counting this element's own
        // opens and closes, to find where it ends.
        let mut depth = 1;
        let mut pos = tag_end;
        while depth > 0 {
            let Some(lt) = rest[pos..].find('<').map(|i| pos + i) else {
                pos = rest.len();
                break;
            };
            let gt = rest[lt..].find('>').map(|i| lt + i + 1).unwrap_or(rest.len());
            let tag = &rest[lt..gt];
            let closing = tag.starts_with("</");
            let tag_name: String =
                tag.trim_start_matches(['<', '/']).chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if tag_name == name {
                if closing {
                    depth -= 1;
                } else {
                    depth += 1;
                }
            }
            pos = gt;
        }
        rest = &rest[pos..];
    }
    out.push_str(rest);
    out
}

fn strip_scheme<'a>(value: &'a str, scheme: &str) -> Option<&'a str> {
    value.get(..scheme.len()).filter(|p| p.eq_ignore_ascii_case(scheme)).map(|_| &value[scheme.len()..])
}

fn is_web(value: &str) -> bool {
    strip_scheme(value, "https://").is_some() || strip_scheme(value, "http://").is_some()
}

fn is_link(value: &str) -> bool {
    is_web(value) || strip_scheme(value, "mailto:").is_some()
}

fn file_url(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'-' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn file_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// Qt draws images at their natural size, so a 1200px newsletter banner would
/// overflow the reader. Rewrites each <img> (ammonia's normalized output, so
/// attributes are always `name="value"`) with a width/height that fits, and
/// drops images whose source was removed.
fn fit_images(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<img") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = &rest[start..];
            break;
        };
        let tag = &rest[start..start + end + 1];
        rest = &rest[start + end + 1..];
        let src = attr(tag, "src");
        let Some(src) = src else { continue };
        let Some(size) = file_path(&src).and_then(|p| imagesize::size(p).ok()) else { continue };
        let (w, h) = (size.width, size.height);
        if w == 0 || h == 0 {
            continue;
        }
        let wanted = attr(tag, "width").and_then(|v| v.parse::<u32>().ok()).unwrap_or(w as u32);
        let width = wanted.min(w as u32).clamp(1, MAX_IMAGE_WIDTH);
        let height = ((h as u64 * width as u64) / w as u64).max(1);
        out.push_str(&format!("<img src=\"{src}\" width=\"{width}\" height=\"{height}\">"));
    }
    out.push_str(rest);
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')?;
    Some(tag[start..start + end].to_string())
}

/// Plain-text body → rich text with clickable links, keeping line breaks.
pub fn text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str("<div style=\"white-space: pre-wrap\">");
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let quoted = line.starts_with('>');
        if quoted {
            out.push_str("<span class=\"quote\">");
        }
        linkify(line, &mut out);
        if quoted {
            out.push_str("</span>");
        }
    }
    out.push_str("</div>");
    out
}

fn linkify(line: &str, out: &mut String) {
    let mut rest = line;
    loop {
        let next = ["https://", "http://"].iter().filter_map(|p| rest.find(p)).min();
        let Some(start) = next else {
            out.push_str(&escape(rest));
            return;
        };
        out.push_str(&escape(&rest[..start]));
        let tail = &rest[start..];
        let mut end = tail.find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"').unwrap_or(tail.len());
        while end > 0 && tail[..end].ends_with(['.', ',', ')', ';', ':', '!', '?', '\'']) {
            end -= 1;
        }
        let url = &tail[..end];
        if url.len() <= "https://".len() {
            out.push_str(&escape(url));
        } else {
            out.push_str(&format!("<a class=\"u\" href=\"{0}\">{0}</a>", escape(url)));
        }
        rest = &tail[end..];
    }
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(html: &str) -> Clean {
        clean_html(html, &HashMap::new(), None)
    }

    #[test]
    fn strips_active_content() {
        let out = clean(
            r#"<script>alert(1)</script><style>p{}</style><p onclick="x()" style="color:red">hi</p>
               <iframe src="https://evil"></iframe><form action="https://x"><input value="v"></form>
               <a href="javascript:alert(1)">js</a><a href="https://ok.az/x">ok</a>"#,
        )
        .html;
        assert!(!out.contains("script") && !out.contains("alert"), "{out}");
        assert!(!out.contains("style") && !out.contains("onclick"), "{out}");
        assert!(!out.contains("iframe") && !out.contains("form") && !out.contains("input"), "{out}");
        assert!(out.contains("<p>hi</p>"), "{out}");
        assert!(out.contains(r#"<a href="https://ok.az/x">ok</a>"#), "{out}");
        assert!(out.contains("<a>js</a>"), "{out}");
    }

    #[test]
    fn blocks_remote_images_by_default() {
        let out = clean(r#"<p>x</p><img src="https://tracker.example/p.gif"><img src="http://a.b/c.png">"#);
        assert!(!out.html.contains("<img"), "{}", out.html);
        assert_eq!(out.remote_images, vec!["https://tracker.example/p.gif", "http://a.b/c.png"]);
    }

    #[test]
    fn drops_local_and_relative_images() {
        let out = clean(r#"<img src="file:///etc/passwd"><img src="/x.png"><img src="data:image/png;base64,AA">"#);
        assert!(!out.html.contains("<img"), "{}", out.html);
        assert!(out.remote_images.is_empty());
    }

    #[test]
    fn rejects_cid_links_and_unknown_cids() {
        let out = clean(r#"<a href="cid:x">a</a><img src="cid:unknown@x">"#).html;
        assert_eq!(out, "<a>a</a>");
    }

    #[test]
    fn scales_inline_images_to_fit() {
        let dir = std::env::temp_dir().join(format!("omamail-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide image.png");
        // 1x1 PNG header claiming 1200x300.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&1200u32.to_be_bytes());
        png.extend_from_slice(&300u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(&path, &png).unwrap();
        let inline = HashMap::from([("logo@x".to_string(), path.clone())]);
        let out = clean_html(r#"<img src="cid:logo@x" style="x" onerror="y">"#, &inline, None).html;
        assert!(out.contains(r#"width="560" height="140""#), "{out}");
        assert!(out.contains("wide%20image.png"), "{out}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hidden_preheaders_are_removed_with_content() {
        let out = clean(
            r#"<div style="display:none;max-height:0">preview <div>nested</div> text</div><p style="color:red">shown</p>
               <span style="opacity:0">x</span><span style="opacity:0.5">half</span><td style="mso-hide:all">o</td>
               <div>a<div style="visibility: hidden">b<div>c</div></div>d</div>"#,
        )
        .html;
        assert!(!out.contains("preview") && !out.contains("nested") && !out.contains("display"), "{out}");
        assert!(out.contains("<p>shown</p>") && out.contains("<span>half</span>"), "{out}");
        assert!(!out.contains(">x<") && !out.contains(">b<") && !out.contains(">c<"), "{out}");
        assert!(out.contains("<div>ad</div>"), "{out}");
    }

    #[test]
    fn marker_text_in_body_is_not_a_marker() {
        let out = clean(r#"<p>a style="display:none" is text</p><p>keep</p>"#).html;
        assert!(out.contains("keep") && out.contains("is text"), "{out}");
    }

    #[test]
    fn layout_tables_become_a_plain_flow() {
        let out = clean(
            r#"<table><tr><td><table><tr><td><a href="https://a.az/x"> Job </a></td><td>Baku</td></tr></table></td></tr>
               <tr><td><a href="https://t.az/p"><img src="https://t.az/p.gif"></a></td></tr><tr><td> </td></tr></table>"#,
        )
        .html;
        assert!(!out.contains("<table") && !out.contains("<td") && !out.contains("<tr"), "{out}");
        assert!(out.contains(r#"<a href="https://a.az/x">Job</a>"#), "{out}");
        assert!(out.contains("Baku"), "{out}");
        assert!(!out.contains("t.az"), "empty link left behind: {out}");
    }

    #[test]
    fn angle_brackets_in_attributes_cannot_become_markup() {
        let out = clean(
            r#"<a href="https://a.az/?q=<img src=x>" title='"><img src="https://evil">'>t</a>
               <img src="cid:none" alt='"><img src="https://evil2">'>"#,
        )
        .html;
        assert!(!out.contains("evil") && !out.contains("<img"), "{out}");
        assert!(!out.contains("title") && !out.contains("alt"), "{out}");
    }

    #[test]
    fn pre_blocks_keep_their_whitespace() {
        let out = clean("<pre>line1\n    indented\nline3</pre><p>a   b</p>").html;
        assert!(out.contains("<pre>line1\n    indented\nline3</pre>"), "{out}");
        assert!(out.contains("<p>a b</p>"), "{out}");
    }

    #[test]
    fn outlook_blank_lines_stay() {
        let out = clean("<p>Hi,</p><p>&nbsp;</p><p>Body</p>").html;
        assert!(out.contains("<p>Hi,</p><br><p>Body</p>"), "{out}");
        let wrapped = clean(r#"<p>Hi,</p><p class="MsoNormal"><span style="x"><o:p>&nbsp;</o:p></span></p><p>x</p>"#).html;
        assert!(wrapped.contains("<p>Hi,</p><br><p>x</p>"), "{wrapped}");
        let in_cell = clean("<table><tr><td><p>A</p><p>&nbsp;</p></td></tr></table>").html;
        assert!(!in_cell.contains("<br>"), "{in_cell}");
    }

    #[test]
    fn highlighted_code_keeps_indentation() {
        let out = clean("<pre>def f():\n<span>    </span>return 1<span>\n</span>x = 2</pre>").html;
        assert!(out.contains("def f():\n<span>    </span>return 1<span>\n</span>x = 2"), "{out}");
    }

    #[test]
    fn spacer_tables_do_not_add_blank_lines() {
        let out = clean(
            "<table><tr><td>A</td></tr></table><table><tr><td>&nbsp;</td></tr></table>\
             <table><tr><td>B</td></tr></table><table><tr><td>&nbsp;</td></tr></table>",
        )
        .html;
        assert_eq!(out.matches("<br>").count(), 1, "{out}");
        assert!(!out.trim_end().ends_with("<br>"), "{out}");
    }

    #[test]
    fn deep_empty_nesting_is_linear() {
        let depth = 5000;
        let html = "<div><span>".repeat(depth) + " " + &"</span></div>".repeat(depth) + "<p>end</p>";
        let started = std::time::Instant::now();
        let out = tidy(&html);
        assert!(started.elapsed().as_millis() < 500, "took {:?}", started.elapsed());
        assert!(out.contains("<p>end</p>") && !out.contains("<span>"), "{}", &out[..out.len().min(200)]);
    }

    #[test]
    fn cards_are_separated_once() {
        let out = clean(
            "<table><tr><td><table><tr><td>A</td></tr></table></td></tr></table>\
             <table><tr><td><table><tr><td>B</td></tr></table></td></tr></table><h1>T</h1>",
        )
        .html;
        // A|B and B|T are card boundaries; the nested closes add nothing.
        assert_eq!(out.matches("<br>").count(), 2, "{out}");
        assert!(out.contains("<h3>T</h3>") && !out.contains("\u{1}"), "{out}");
        assert!(out.find('A') < out.find("<br>") && out.find("<br>") < out.find('B'), "{out}");
    }

    #[test]
    fn empty_wrappers_are_dropped_but_content_kept() {
        let out = clean("<div><p> </p><span>&nbsp;</span><b>x</b><br></div><p><i> </i></p><b><br></b>").html;
        assert_eq!(out.trim(), "<div> <b>x</b><br></div> <b><br></b>", "{out}");
    }

    #[test]
    fn plain_text_links_are_escaped() {
        let html = text_to_html("see https://a.az/x?q=1&b=2. <b>\n> quoted");
        assert!(html.contains(r#"<a class="u" href="https://a.az/x?q=1&amp;b=2">"#), "{html}");
        assert!(html.contains("&lt;b&gt;"), "{html}");
        assert!(html.contains(r#"<span class="quote">&gt; quoted</span>"#), "{html}");
    }

    #[test]
    fn link_text_cannot_break_out_of_href() {
        let html = text_to_html(r#"https://a.az/"onmouseover="x"#);
        assert!(html.contains(r#"href="https://a.az/""#), "{html}");
        assert!(!html.contains(r#""onmouseover"#), "{html}");
    }
}

#[cfg(test)]
mod perf {
    #[test]
    fn deep_hidden_element_is_removed_in_linear_time() {
        let depth = 20000;
        // Already in ammonia's normalized form, so only remove_hidden is timed.
        let html = r#"<div style="display:none">"#.to_string() + &"<div>x".repeat(depth) + &"</div>".repeat(depth) + "</div><p>end</p>";
        let started = std::time::Instant::now();
        let out = super::remove_hidden(&html);
        assert_eq!(out, "<p>end</p>");
        assert!(started.elapsed().as_millis() < 200, "took {:?}", started.elapsed());
    }
}
