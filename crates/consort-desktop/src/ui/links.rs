//! The link policy for message text, and file URLs.
//!
//! A message is text from a person or from a model. A link in it can point
//! anywhere: a local file, a network share, or a custom scheme that starts a
//! program. The app opens only web and email links. When the words of a link
//! do not match where it goes, the app shows the real address and asks first.

use std::path::Path;

/// The schemes that the app opens.
const OPEN_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// What to do with a clicked link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAction {
    /// Open this address now.
    Open(String),
    /// Show this address and ask before it opens.
    Confirm(String),
    /// Do not open the link.
    Block(Blocked),
}

/// Why the app does not open a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// The scheme is not one that the app opens, such as `file` or `smb`.
    /// The scheme is in lower case.
    Scheme(String),
    /// The link has no scheme, or no host or address after it. The app has
    /// no base address to resolve it against.
    Incomplete,
}

impl Blocked {
    /// The notice for a link that did not open. It says what happened and
    /// what to do next.
    pub fn notice(&self) -> String {
        match self {
            Blocked::Scheme(scheme) => format!(
                "Consort did not open the link, because it opens only web and email links, and this one starts with {scheme}. Copy the link and open it yourself if you trust it."
            ),
            Blocked::Incomplete => "Consort did not open the link, because it has no full web address. Ask for the full address.".to_owned(),
        }
    }
}

/// Decides what to do with a link. `label` is the text that the reader
/// sees, or `None` when it is not known. `target` is where the link goes.
///
/// - Only `http`, `https` and `mailto` open. The scheme is not case
///   sensitive. Spaces, tabs and line breaks around or inside the scheme do
///   not hide it.
/// - A link with no scheme (a relative or protocol-relative link) is blocked.
/// - A link opens at once only when its label is the same address as its
///   target. In every other case the app asks first.
pub fn link_action(label: Option<&str>, target: &str) -> LinkAction {
    let url = clean(target);
    let Some(scheme) = scheme_of(&url) else {
        return LinkAction::Block(Blocked::Incomplete);
    };
    let scheme = scheme.to_ascii_lowercase();
    if !OPEN_SCHEMES.contains(&scheme.as_str()) {
        return LinkAction::Block(Blocked::Scheme(scheme));
    }
    let rest = &url[scheme.len() + 1..];
    let complete = if scheme == "mailto" {
        !rest.trim().is_empty()
    } else {
        rest.strip_prefix("//")
            .is_some_and(|host| !host.is_empty() && !host.starts_with(['/', '?', '#']))
    };
    if !complete {
        return LinkAction::Block(Blocked::Incomplete);
    }
    match label {
        Some(label) if same_address(label, &url) => LinkAction::Open(url),
        _ => LinkAction::Confirm(url),
    }
}

/// The link as a browser reads it: tabs and line breaks removed, and control
/// characters and spaces cut from both ends.
fn clean(target: &str) -> String {
    let without_breaks: String = target
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    without_breaks
        .trim_matches(|c: char| c.is_ascii_control() || c == ' ')
        .to_owned()
}

/// The scheme at the start of a link, if it has one: a letter, then letters,
/// digits, `+`, `-` or `.`, then `:`.
fn scheme_of(url: &str) -> Option<&str> {
    let end = url.find(':')?;
    let scheme = &url[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    let valid = first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then_some(scheme)
}

/// True when the words of a link name the address it goes to. The scheme,
/// a leading `www.`, a closing slash and case do not matter.
pub fn same_address(label: &str, target: &str) -> bool {
    fn plain(text: &str) -> String {
        let lower = text.trim().to_ascii_lowercase();
        let lower = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
            .or_else(|| lower.strip_prefix("mailto:"))
            .unwrap_or(&lower);
        let lower = lower.strip_prefix("www.").unwrap_or(lower);
        lower.trim_end_matches('/').to_owned()
    }
    let label = plain(label);
    !label.is_empty() && label == plain(target)
}

/// Decides what to do with a link that the reader clicked in `markdown`.
/// The text view gives the app only the address, not the link that was
/// clicked, so this reads every place where the address is a link in the
/// Markdown. The link opens at once only when each of them shows the address
/// as its own words:
///
/// - an inline link `[label](address)` whose label is the address,
/// - an autolink `<address>`,
/// - a bare address in the text.
///
/// A mention of the address that is not one of these never counts as the
/// link showing it. A raw HTML link, a reference link, a link with other
/// words, and an address in a title or in code make the app ask first. The
/// same goes for an address that the scan does not find at all.
pub fn click_action(markdown: &str, target: &str) -> LinkAction {
    let wanted = clean(target);
    if shows_only_its_own_address(markdown, &wanted) {
        link_action(Some(&wanted), target)
    } else {
        link_action(None, target)
    }
}

/// Replaces the bytes `from..to` with spaces. Every range starts and ends at
/// an ASCII byte, so the text stays valid UTF-8.
fn blank(bytes: &mut [u8], from: usize, to: usize) {
    let to = to.min(bytes.len());
    for b in &mut bytes[from..to] {
        *b = b' ';
    }
}

/// The address of an inline link or an autolink, as a browser reads it.
fn same_link(address: &str, target: &str) -> bool {
    let address = clean(address);
    address == target || Some(address.as_str()) == target.strip_prefix("mailto:")
}

/// True when `target` is a link in `markdown` only in forms that show the
/// address as the words of the link, and at least one such link exists.
fn shows_only_its_own_address(markdown: &str, target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    let mut text = markdown.as_bytes().to_vec();
    let mut shown = 0usize;
    let mut risky = false;

    // Reference definitions (`[r]: address "title"`). A reference link takes
    // its address from one, and its words are somewhere else. The address
    // may sit on the next line.
    let mut offset = 0;
    let mut definition_open = false;
    for line in markdown.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        let starts = trimmed.starts_with('[') && trimmed.contains("]:");
        let after_colon = trimmed.split_once("]:").map(|(_, rest)| rest);
        let continues = definition_open && !body.trim().is_empty();
        if starts || continues {
            let rest = if starts {
                after_colon.unwrap_or("")
            } else {
                body
            };
            if rest.contains(target) {
                risky = true;
            }
            definition_open = starts && rest.trim().is_empty();
            blank(&mut text, offset, offset + body.len());
        } else if !body.trim().is_empty() {
            definition_open = false;
        }
        offset += line.len();
    }

    // Angle brackets: an autolink `<address>`, or raw HTML. Any mention of
    // the address in a tag (an `href`, a `title`, anything) is risky, because
    // the words of an HTML link are not in the tag.
    let mut at = 0;
    while let Some(found) = markdown.as_bytes()[at..].iter().position(|b| *b == b'<') {
        let open = at + found;
        at = open + 1;
        if text.get(open) != Some(&b'<') {
            continue; // Blanked already.
        }
        let rest = &markdown[open + 1..];
        let next = rest.chars().next();
        let inner_end = rest.find('>');
        if let (Some(end), Some(first)) = (inner_end, next) {
            let inner = &rest[..end];
            let is_autolink = !inner.is_empty()
                && !inner.contains(|c: char| c.is_whitespace() || c == '<')
                && (scheme_of(inner).is_some() || (inner.contains('@') && first != '@'));
            if is_autolink {
                if same_link(inner, target) {
                    shown += 1;
                }
                blank(&mut text, open, open + 1 + end + 1);
                at = open + 1 + end + 1;
                continue;
            }
        }
        if matches!(next, Some(c) if c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?')) {
            let end = if rest.starts_with("!--") {
                rest.find("-->").map(|i| open + 1 + i + 3)
            } else {
                inner_end.map(|i| open + 1 + i + 1)
            }
            .unwrap_or(markdown.len());
            if markdown[open..end].contains(target) {
                risky = true;
            }
            blank(&mut text, open, end);
            at = end;
        }
    }

    // Inline links `[label](address "title")`. The whole link is read, so
    // the label and the title are not counted as mentions of their own.
    let mut at = 0;
    while let Some(found) = markdown[at..].find("](") {
        let close = at + found;
        at = close + 2;
        if text.get(close) != Some(&b']') {
            continue; // Inside raw HTML or a definition.
        }
        let Some((address, used)) = link_address(&markdown[at..]) else {
            continue;
        };
        let Some(end) = markdown[at + used..].find(')').map(|i| at + used + i + 1) else {
            continue;
        };
        let Some(open) = label_start(markdown.as_bytes(), close) else {
            continue;
        };
        if same_link(&address, target) {
            if same_address(&markdown[open + 1..close], target) {
                shown += 1;
            } else {
                risky = true;
            }
        }
        blank(&mut text, open, end);
    }

    // What is left is plain text. A bare address with a boundary on both
    // sides is a link that shows itself. An address after a character that
    // can open a destination, an attribute, a label or code is a place that
    // this scan does not understand, so it asks.
    let rest = String::from_utf8_lossy(&text).into_owned();
    let without_scheme = target.strip_prefix("mailto:").unwrap_or(target);
    let mut wanted = vec![target];
    if without_scheme != target {
        wanted.push(without_scheme);
    }
    for wanted in wanted {
        for (at, _) in rest.match_indices(wanted) {
            let before = rest[..at].chars().next_back();
            let after = rest[at + wanted.len()..].chars().next();
            let bare_before =
                before.is_none_or(|c| c.is_whitespace() || matches!(c, '*' | '_' | '~'));
            let bare_after = after.is_none_or(|c| {
                c.is_whitespace()
                    || matches!(c, '*' | '_' | '~' | '.' | ',' | ';' | ':' | '!' | '?' | ')')
            });
            if bare_before && bare_after {
                shown += 1;
            } else if before.is_some_and(|c| matches!(c, '(' | '[' | '"' | '\'' | '=' | '<' | '`'))
            {
                risky = true;
            }
        }
    }
    !risky && shown > 0
}

/// The byte index of the `[` that opens the label that ends at `close`.
fn label_start(bytes: &[u8], close: usize) -> Option<usize> {
    let mut depth = 0usize;
    for i in (0..close).rev() {
        if i > 0 && bytes[i - 1] == b'\\' {
            continue;
        }
        match bytes[i] {
            b']' => depth += 1,
            b'[' if depth == 0 => return Some(i),
            b'[' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Reads the address of an inline link after `](`: plain, or in `<...>`,
/// with an optional title. Returns the address and the bytes read.
fn link_address(after: &str) -> Option<(String, usize)> {
    let trimmed = after.trim_start();
    let skipped = after.len() - trimmed.len();
    if let Some(inner) = trimmed.strip_prefix('<') {
        let end = inner.find('>')?;
        return Some((inner[..end].to_owned(), skipped + end + 2));
    }
    let mut depth = 0usize;
    let mut end = trimmed.len();
    for (i, c) in trimmed.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => {
                end = i;
                break;
            }
            ')' => depth -= 1,
            c if c.is_whitespace() => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    (end > 0).then(|| (trimmed[..end].to_owned(), skipped + end))
}

/// A `file:` URL for a path, with every byte that is not safe percent
/// encoded: `/Users/me/Library/Application Support/x` becomes
/// `file:///Users/me/Library/Application%20Support/x`.
pub fn file_url(path: &Path) -> String {
    file_url_from(&path.to_string_lossy(), cfg!(windows))
}

fn file_url_from(path: &str, windows: bool) -> String {
    let path = if windows {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    };
    // A Windows network path `//server/share` keeps its host: `file://server/share`.
    // A drive path `C:/Users` gets a slash in front.
    let network = windows && path.starts_with("//");
    let mut url = String::from(if network { "file:" } else { "file://" });
    if !path.starts_with('/') {
        url.push('/');
    }
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(char::from(byte))
            }
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(url: &str) -> LinkAction {
        LinkAction::Open(url.to_owned())
    }

    fn confirm(url: &str) -> LinkAction {
        LinkAction::Confirm(url.to_owned())
    }

    fn scheme(name: &str) -> LinkAction {
        LinkAction::Block(Blocked::Scheme(name.to_owned()))
    }

    #[test]
    fn web_and_email_links_with_the_same_words_open_at_once() {
        assert_eq!(
            link_action(Some("https://example.com/docs"), "https://example.com/docs"),
            open("https://example.com/docs")
        );
        assert_eq!(
            link_action(Some("http://example.com"), "http://example.com"),
            open("http://example.com")
        );
        assert_eq!(
            link_action(Some("me@example.com"), "mailto:me@example.com"),
            open("mailto:me@example.com")
        );
    }

    #[test]
    fn a_shortened_label_for_the_same_address_opens_at_once() {
        assert_eq!(
            link_action(Some("example.com"), "https://example.com/"),
            open("https://example.com/")
        );
        assert_eq!(
            link_action(Some("www.example.com/docs"), "https://example.com/docs"),
            open("https://example.com/docs")
        );
    }

    #[test]
    fn a_label_that_names_another_place_asks_first() {
        assert_eq!(
            link_action(Some("Click here"), "https://example.com"),
            confirm("https://example.com")
        );
        // The classic trick: the words look like a safe address.
        assert_eq!(
            link_action(Some("https://bank.example"), "https://evil.example"),
            confirm("https://evil.example")
        );
        assert_eq!(
            link_action(Some(""), "https://example.com"),
            confirm("https://example.com")
        );
        assert_eq!(
            link_action(None, "https://example.com"),
            confirm("https://example.com")
        );
    }

    #[test]
    fn the_scheme_is_not_case_sensitive() {
        assert_eq!(
            link_action(Some("HTTPS://EXAMPLE.COM"), "HTTPS://Example.com"),
            open("HTTPS://Example.com")
        );
        assert_eq!(
            link_action(Some("x"), "MailTo:me@example.com"),
            confirm("MailTo:me@example.com")
        );
        assert_eq!(
            link_action(Some("x"), "JavaScript:alert(1)"),
            scheme("javascript")
        );
        assert_eq!(link_action(Some("x"), "FILE:///etc/passwd"), scheme("file"));
    }

    #[test]
    fn spaces_and_breaks_do_not_hide_the_scheme() {
        assert_eq!(
            link_action(Some("https://example.com"), "  https://example.com \n"),
            open("https://example.com")
        );
        assert_eq!(
            link_action(Some("x"), "java\tscript:alert(1)"),
            scheme("javascript")
        );
        assert_eq!(
            link_action(Some("x"), "\u{1}javascript:alert(1)"),
            scheme("javascript")
        );
        assert_eq!(
            link_action(Some("x"), "ja\nvascript:alert(1)"),
            scheme("javascript")
        );
        assert_eq!(
            link_action(Some("x"), " java script:alert(1)"),
            LinkAction::Block(Blocked::Incomplete)
        );
    }

    #[test]
    fn local_file_network_and_custom_schemes_are_blocked() {
        for (target, name) in [
            ("javascript:alert(1)", "javascript"),
            ("file:///Users/me/secret.txt", "file"),
            ("smb://server/share", "smb"),
            ("vnc://host", "vnc"),
            ("ssh://host", "ssh"),
            ("tel:+15555550100", "tel"),
            ("data:text/html,<b>x</b>", "data"),
            ("consort-update://run", "consort-update"),
            ("C:\\Windows\\System32\\calc.exe", "c"),
            ("ftp://example.com", "ftp"),
        ] {
            assert_eq!(link_action(Some(target), target), scheme(name), "{target}");
        }
    }

    #[test]
    fn links_with_no_scheme_or_no_address_are_blocked() {
        for target in [
            "",
            "   ",
            "docs/readme.md",
            "/etc/passwd",
            "./a",
            "#section",
            "?q=1",
            "//evil.example/path",
            "example.com",
            "http:example.com",
            "https://",
            "https:///path",
            "mailto:",
            "not a scheme: x",
        ] {
            assert_eq!(
                link_action(Some(target), target),
                LinkAction::Block(Blocked::Incomplete),
                "{target:?}"
            );
        }
    }

    #[test]
    fn the_notice_says_what_happened_and_what_to_do() {
        let text = Blocked::Scheme("file".into()).notice();
        assert!(text.contains("file"));
        assert!(text.contains("Copy the link"));
        assert!(Blocked::Incomplete.notice().contains("full web address"));
    }

    #[test]
    fn a_click_finds_the_words_of_an_inline_link() {
        let md = "See [the docs](https://example.com/docs) now.";
        assert_eq!(
            click_action(md, "https://example.com/docs"),
            confirm("https://example.com/docs")
        );
        let md = "See [https://example.com/docs](https://example.com/docs).";
        assert_eq!(
            click_action(md, "https://example.com/docs"),
            open("https://example.com/docs")
        );
    }

    #[test]
    fn one_link_with_other_words_asks_even_when_another_matches() {
        let md = "[https://good.example](https://evil.example) and [https://evil.example](https://evil.example)";
        assert_eq!(
            click_action(md, "https://evil.example"),
            confirm("https://evil.example")
        );
    }

    #[test]
    fn a_label_with_brackets_and_an_address_with_parentheses_work() {
        let md = "[a [b] c](https://example.com/a_(b) \"A title\")";
        assert_eq!(
            click_action(md, "https://example.com/a_(b)"),
            confirm("https://example.com/a_(b)")
        );
        let md = "[x](<https://example.com/a b>)";
        assert_eq!(
            click_action(md, "https://example.com/a b"),
            confirm("https://example.com/a b")
        );
    }

    #[test]
    fn a_bare_address_and_an_autolink_show_their_own_words() {
        assert_eq!(
            click_action("Read https://example.com/x.", "https://example.com/x"),
            open("https://example.com/x")
        );
        assert_eq!(
            click_action("Read <https://example.com/x> now", "https://example.com/x"),
            open("https://example.com/x")
        );
        assert_eq!(
            click_action("Write to <me@example.com>", "mailto:me@example.com"),
            open("mailto:me@example.com")
        );
    }

    #[test]
    fn a_reference_link_asks_first() {
        let md = "Go [here][1].\n\n[1]: https://example.com/x";
        assert_eq!(
            click_action(md, "https://example.com/x"),
            confirm("https://example.com/x")
        );
    }

    // The cases of the review probe (review cycle 2, finding 2). Each one
    // showed a label that is not the address, or no label at all, and gave
    // Open with no question, because the address stood somewhere else in the
    // text as its own words.
    const EVIL: &str = "https://evil.example/login";

    #[test]
    fn a_raw_html_link_with_a_padded_address_asks_first() {
        let md =
            r#"Docs: <a href=" https://evil.example/login ">https://github.com/anthropics</a>"#;
        assert_eq!(
            click_action(md, " https://evil.example/login "),
            confirm(EVIL)
        );
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
    }

    #[test]
    fn a_raw_html_link_asks_first_even_without_padding() {
        let md = r#"<a href="https://evil.example/login">https://github.com/anthropics</a>"#;
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
        // Even when the words of the HTML link are the address.
        let same = r#"<a href="https://evil.example/login">https://evil.example/login</a>"#;
        assert_eq!(click_action(same, EVIL), confirm(EVIL));
    }

    #[test]
    fn a_title_attribute_with_the_address_does_not_make_it_show_itself() {
        let md =
            r#"<a href="https://evil.example/login" title=" https://evil.example/login ">Docs</a>"#;
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
    }

    #[test]
    fn a_reference_link_whose_label_is_another_address_asks_first() {
        let md = "[https://github.com/anthropics][r]\n\n[r]: https://evil.example/login";
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
    }

    #[test]
    fn a_decoy_line_does_not_make_a_reference_link_show_itself() {
        let md = "[https://github.com/anthropics][r]\n\nIf it fails, copy https://evil.example/login\n\n[r]: https://evil.example/login";
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
        // The definition may put its address on the next line.
        let wrapped =
            "[Docs][r]\n\nCopy https://evil.example/login\n\n[r]:\n    https://evil.example/login";
        assert_eq!(click_action(wrapped, EVIL), confirm(EVIL));
    }

    #[test]
    fn an_inline_link_with_another_address_as_its_words_asks_first() {
        let md = "[https://github.com/anthropics](https://evil.example/login)";
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
        // A bare mention of the address in the same message does not help.
        let decoy = format!("{md}\n\nCopy https://evil.example/login");
        assert_eq!(click_action(&decoy, EVIL), confirm(EVIL));
    }

    #[test]
    fn a_mention_in_code_or_in_a_title_does_not_count() {
        let md = "[Docs](https://evil.example/login \"https://evil.example/login\")";
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
        let code =
            "Run `https://evil.example/login` and read [the docs](https://evil.example/login).";
        assert_eq!(click_action(code, EVIL), confirm(EVIL));
        // An address that the scan does not find asks too.
        assert_eq!(click_action("No link here.", EVIL), confirm(EVIL));
        assert_eq!(click_action("", EVIL), confirm(EVIL));
    }

    #[test]
    fn links_that_show_their_own_address_still_open_at_once() {
        // Inline, autolink and bare, alone or together.
        let inline = "[https://evil.example/login](https://evil.example/login)";
        assert_eq!(click_action(inline, EVIL), open(EVIL));
        assert_eq!(
            click_action("<https://evil.example/login>", EVIL),
            open(EVIL)
        );
        assert_eq!(
            click_action("Go to https://evil.example/login now", EVIL),
            open(EVIL)
        );
        let all = "[https://evil.example/login](https://evil.example/login \"A title\")\n\n<https://evil.example/login>\n\nOr **https://evil.example/login**.";
        assert_eq!(click_action(all, EVIL), open(EVIL));
        // A shortened label for the same address.
        assert_eq!(
            click_action("[evil.example/login](https://evil.example/login)", EVIL),
            open(EVIL)
        );
    }

    #[test]
    fn one_link_that_hides_its_address_makes_every_link_to_it_ask() {
        let md = "https://evil.example/login and [click here](https://evil.example/login)";
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
        let html = "<https://evil.example/login> and <a href=\"https://evil.example/login\">go</a>";
        assert_eq!(click_action(html, EVIL), confirm(EVIL));
    }

    #[test]
    fn another_address_in_the_message_does_not_matter() {
        let md = "[the docs](https://good.example/docs) and https://evil.example/login";
        assert_eq!(click_action(md, EVIL), open(EVIL));
        assert_eq!(
            click_action(md, "https://good.example/docs"),
            confirm("https://good.example/docs")
        );
        // A longer address that starts with this one is another address.
        let longer = "https://evil.example/login/reset and [x](https://evil.example/login)";
        assert_eq!(click_action(longer, EVIL), confirm(EVIL));
        let only_longer = "See https://evil.example/login/reset";
        assert_eq!(click_action(only_longer, EVIL), confirm(EVIL));
    }

    #[test]
    fn non_ascii_text_around_a_link_does_not_break_the_scan() {
        let md = "Voil\u{e0} \u{2192} <a href=\"https://evil.example/login\">caf\u{e9}</a> \u{1F600} done";
        assert_eq!(click_action(md, EVIL), confirm(EVIL));
        let ok = "Voil\u{e0} https://evil.example/login \u{1F600}";
        assert_eq!(click_action(ok, EVIL), open(EVIL));
    }

    #[test]
    fn a_click_on_a_blocked_scheme_never_opens() {
        let md = "[open it](file:///etc/passwd) or [x](javascript:alert(1))";
        assert_eq!(click_action(md, "file:///etc/passwd"), scheme("file"));
        assert_eq!(
            click_action(md, "javascript:alert(1)"),
            scheme("javascript")
        );
    }

    #[test]
    fn file_urls_encode_spaces_and_other_unsafe_bytes() {
        assert_eq!(
            file_url_from(
                "/Users/me/Library/Application Support/consort/themes",
                false
            ),
            "file:///Users/me/Library/Application%20Support/consort/themes"
        );
        assert_eq!(
            file_url_from("/tmp/a#b?c%d+e/ü", false),
            "file:///tmp/a%23b%3Fc%25d%2Be/%C3%BC"
        );
        assert_eq!(file_url_from("/plain/path", false), "file:///plain/path");
    }

    #[test]
    fn file_urls_for_windows_paths_use_forward_slashes() {
        assert_eq!(
            file_url_from("C:\\Users\\Me\\AppData\\Roaming\\consort", true),
            "file:///C:/Users/Me/AppData/Roaming/consort"
        );
        assert_eq!(
            file_url_from("\\\\server\\share\\my themes", true),
            "file://server/share/my%20themes"
        );
    }

    #[test]
    fn a_backslash_in_a_unix_path_stays_a_byte_of_the_name() {
        assert_eq!(file_url_from("/tmp/a\\b", false), "file:///tmp/a%5Cb");
    }
}
