//! Finds the one-time code in a verification mail ("295092 is your
//! NeuroBridge code", "Doğrulama kodunuz: 123 456"), so the notification can
//! copy it with one click.
//!
//! Digit codes of 4–8 digits are considered within a short distance of a
//! code-ish word, so order numbers, prices, times, dates and years in
//! ordinary mail are left alone. Failing that, a letters-and-digits token
//! right after such a word ("…use the following code: c4491f") counts.

const KEYWORDS: &[&str] = &[
    "code", "kod", "код", "otp", "passcode", "pin", "verification", "verify", "one-time", "one time",
    "doğrulama", "dogrulama", "təsdiq", "tesdiq", "giriş", "sign-in", "sign in", "login", "log in",
    "security", "confirm", "2fa", "two-factor",
];

/// Max characters between a keyword and the code.
const NEAR: usize = 60;

/// The body is only searched when the subject itself reads like a
/// verification mail: newsletters and notifications mention "code" next to
/// commit hashes, order numbers and the like far too often.
pub fn find_code(subject: &str, body: &str) -> Option<String> {
    // Zero-width characters inside "12\u{200b}3456" would split the code.
    let visible = |c: &char| !matches!(c, '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{ad}');
    let subject: String = subject.chars().filter(visible).take(500).collect();
    let body: String = body.chars().filter(visible).take(3000).collect();
    find_in(&subject).or_else(|| {
        if !has_keyword(&subject) {
            return None;
        }
        find_in(&body)
    })
}

/// Whether a subject reads like a verification mail, i.e. whether its body
/// is worth fetching at all.
pub fn has_keyword(subject: &str) -> bool {
    !keyword_positions(&lowercase(subject)).is_empty()
}

/// One char per char, so positions in the lowered text match the original:
/// `str::to_lowercase` turns "İ" into two chars.
fn lowercase(text: &str) -> Vec<char> {
    text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

fn find_in(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    find_aligned(&chars, &lowercase(text))
}

fn find_aligned(chars: &[char], lower: &[char]) -> Option<String> {
    let keyword_spots = keyword_positions(lower);
    if keyword_spots.is_empty() {
        return None;
    }
    let mut best: Option<(usize, String)> = None;
    for (start, end, digits) in candidates(chars) {
        let distance = keyword_spots
            .iter()
            .map(|&(ks, ke)| if ke <= start { start - ke } else { ks.saturating_sub(end) })
            .min()
            .unwrap_or(usize::MAX);
        if distance <= NEAR && best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((distance, digits));
        }
    }
    best.map(|(_, code)| code).or_else(|| mixed_after_keyword(chars, &keyword_spots))
}

/// A 5–10 character ASCII token of letters and digits (both present),
/// standing alone after a keyword within NEAR characters.
fn mixed_after_keyword(chars: &[char], spots: &[(usize, usize)]) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_alphanumeric() || (i > 0 && chars[i - 1].is_alphanumeric()) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_alphanumeric() {
            i += 1;
        }
        let token = &chars[start..i];
        // A value on its own: after a colon, a sentence end or a line break
        // ("…30 minutes: c4491f", "…website. 03c060"), never mid-sentence
        // ("Sign in to acme42.com", "on Pixel7a") or inside a name or path.
        let before = chars[..start].iter().rev().find(|c| !c.is_whitespace() || **c == '\n');
        let set_apart = start == 0 || matches!(before, None | Some(':' | '.' | '\n'));
        let clean_before = start == 0 || chars[start - 1].is_whitespace() || chars[start - 1] == ':';
        let clean_after = match chars.get(i) {
            None => true,
            Some('.' | ',') => !chars.get(i + 1).is_some_and(|c| c.is_alphanumeric()),
            Some(c) => !c.is_alphanumeric() && !matches!(c, '-' | '_' | '@' | '/'),
        };
        let standalone = set_apart && clean_before && clean_after;
        let mixed = token.iter().any(char::is_ascii_digit) && token.iter().any(char::is_ascii_alphabetic);
        if !standalone || !mixed || !(5..=10).contains(&token.len()) {
            continue;
        }
        // The token must not be a keyword itself ("2fa") and must follow one.
        let distance = spots
            .iter()
            .filter(|&&(ks, ke)| ke <= start && !(ks..ke).contains(&start))
            .map(|&(_, ke)| start - ke)
            .min();
        if let Some(d) = distance.filter(|d| *d <= NEAR)
            && best.as_ref().is_none_or(|(b, _)| d < *b)
        {
            best = Some((d, token.iter().collect()));
        }
    }
    best.map(|(_, code)| code)
}

fn keyword_positions(lower: &[char]) -> Vec<(usize, usize)> {
    let mut spots = Vec::new();
    for kw in KEYWORDS {
        let k: Vec<char> = kw.chars().collect();
        let mut i = 0;
        while i + k.len() <= lower.len() {
            if lower[i..i + k.len()] == k[..] {
                // Word start only: "pin" must not match inside "shopping".
                let before_ok = i == 0 || !lower[i - 1].is_alphanumeric();
                if before_ok {
                    spots.push((i, i + k.len()));
                }
                i += k.len();
            } else {
                i += 1;
            }
        }
    }
    spots
}

/// (start, end, digits) of each plausible code: a run of 4–8 digits, which
/// may be split once or twice by a single space or hyphen into groups of
/// 3–4 ("123 456", "1234-5678"), standing alone as a token.
fn candidates(chars: &[char]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() || glued_before(chars, i) {
            i += 1;
            continue;
        }
        let start = i;
        let mut groups: Vec<String> = Vec::new();
        let mut j = i;
        loop {
            let g_start = j;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            groups.push(chars[g_start..j].iter().collect());
            let sep = chars.get(j).copied();
            let next_is_digit = chars.get(j + 1).is_some_and(|c| c.is_ascii_digit());
            // An HTML `&nbsp;` between groups arrives as U+00A0.
            if groups.len() < 3 && matches!(sep, Some(' ' | '-' | '\u{a0}')) && next_is_digit {
                j += 1;
                continue;
            }
            break;
        }
        i = j.max(start + 1);
        let grouped_ok = groups.len() == 1 || groups.iter().all(|g| (3..=4).contains(&g.len()));
        // "551902 15 min": the run did not form a grouped code, but its first
        // group may still be one on its own.
        let (end, digits) = if grouped_ok {
            (j, groups.concat())
        } else {
            (start + groups[0].len(), groups[0].clone())
        };
        if glued_after(chars, end) || !(4..=8).contains(&digits.len()) {
            continue;
        }
        if digits.len() == 4 && digits.parse::<u32>().is_ok_and(|y| (1900..=2099).contains(&y)) {
            continue; // a year
        }
        out.push((start, end, digits));
    }
    out
}

/// Characters that make a digit run part of something else: a word, a
/// price, a time, a date, a reference like "#123456".
fn is_glued(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '$' | '€' | '£' | '₼' | '%' | '/' | '#' | '+' | '_' | '@')
}

/// A period or comma glues only between digits (1.25, 1,000, 1.2.3456);
/// at the end of a sentence it is just punctuation.
fn glued_before(chars: &[char], i: usize) -> bool {
    match i.checked_sub(1).map(|p| chars[p]) {
        // "10:30", "1.25", "1,000" are glued; "code:482913" is not.
        Some('.' | ',' | ':') => i >= 2 && chars[i - 2].is_ascii_digit(),
        Some(c) => is_glued(c),
        None => false,
    }
}

fn glued_after(chars: &[char], end: usize) -> bool {
    match chars.get(end) {
        Some('.' | ',' | ':') => chars.get(end + 1).is_some_and(|c| c.is_ascii_digit()),
        Some(c) => is_glued(*c),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::find_code;

    #[test]
    fn finds_codes_in_subjects() {
        assert_eq!(find_code("295092 is your NeuroBridge code", ""), Some("295092".into()));
        assert_eq!(find_code("Your Google verification code is 482913", ""), Some("482913".into()));
        assert_eq!(find_code("Doğrulama kodunuz: 123 456", ""), Some("123456".into()));
        assert_eq!(find_code("Login code: 7731", ""), Some("7731".into()));
        assert_eq!(find_code("Your code is 552901.", ""), Some("552901".into()));
        assert_eq!(find_code("GİRİŞ KODU: 123456", ""), Some("123456".into()));
        assert_eq!(find_code("İnternet bank: Your Code 482913", ""), Some("482913".into()));
        assert_eq!(find_code("Verification code:482913", ""), Some("482913".into()));
        assert_eq!(find_code("Kod:4821", ""), Some("4821".into()));
        assert_eq!(find_code("Your code 551902 15 min", ""), Some("551902".into()));
        assert_eq!(find_code("Your code 1234\u{a0}5678", ""), Some("12345678".into()));
        assert_eq!(find_code("Your code 12\u{200b}3456", ""), Some("123456".into()));
        assert_eq!(find_code("Ваш код 4821-9934", ""), Some("48219934".into()));
    }

    #[test]
    fn falls_back_to_the_body() {
        let body = "Hi,\n\nUse the following one-time passcode to sign in:\n\n  603 118\n\nIt expires in 10 minutes.";
        assert_eq!(find_code("Sign in to Example", body), Some("603118".into()));
    }

    #[test]
    fn ignores_ordinary_numbers() {
        for subject in [
            "Order #123456 has shipped",
            "Your invoice for 2026",
            "Meeting moved to 10:30 for the code review",
            "Price drop: now $1299",
            "Release 1.2.3456 notes",
            "Shopping list 4521",
            "Call me at +994501234567",
            "Ticket ABC123456 updated",
            "Total 1,250.75 code 10.99",
        ] {
            assert_eq!(find_code(subject, ""), None, "{subject}");
        }
    }

    #[test]
    fn mixed_codes_after_a_keyword() {
        let body = "To verify your account, please use the following code to enable your new device — it will expire in 30 minutes: c4491f That wasn't me!";
        assert_eq!(find_code("Your confirmation code", body), Some("c4491f".into()));
        assert_eq!(find_code("Your code: AB12CD", ""), Some("AB12CD".into()));
        let spaceship = "Copy the temporary authentication code into the verification form on our website. 03c060 It expires soon.";
        assert_eq!(find_code("Please verify your account", spaceship), Some("03c060".into()));
        for none in [
            "Enable 2fa today",
            "a1b2c3 is not near anything",
            "Your code for user john_doe2",
            "Login code for john_smith42",
            "Sign in to acme42.com",
            "New sign-in on Pixel7a",
        ] {
            assert_eq!(find_code(none, ""), None, "{none}");
        }
    }

    #[test]
    fn needs_a_keyword_nearby() {
        let far = format!("Your code is below.{}\n\n123456", " filler".repeat(20));
        assert_eq!(find_code("Your code", &far), None);
        assert_eq!(find_code("Your code", "Your code is below:\n\n123456"), Some("123456".into()));
    }

    #[test]
    fn body_needs_a_verification_subject() {
        let body = "Pushed commit 7bec656 to main. Code owners: 12. Your code review: 482913";
        assert_eq!(find_code("Re: [omacom/marketplace] [Plugin]: Omamail", body), None);
        assert_eq!(find_code("Spaceship order summary", "Promo code COM67 applied, order 551902"), None);
    }

    #[test]
    fn prefers_the_closest_number() {
        let body = "Account 98765432 at 2026. Your verification code is 551 902.";
        assert_eq!(find_code("Your verification code", body), Some("551902".into()));
    }
}
