//! The document that a client or the renderer loads: the page of the
//! assistant with the bootstrap in front of it. Also the messages that the
//! bootstrap sends to the app, and the expressions that the renderer runs.

use std::sync::LazyLock;

use base64::{Engine, engine::general_purpose::STANDARD};
use consort_proto::model::{VisualStyle, clamp_height};
use consort_theme::fonts;
use regex::Regex;

use crate::error::PageError;

/// Who loads the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// The webview of the desktop app. The document gets the size reporter
    /// and the link catcher.
    Webview,
    /// The headless browser of the session host. It measures the page from
    /// outside, so the document has no scripts of the app.
    Renderer,
}

/// A message from the bootstrap of a page to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageMessage {
    /// The height of the page in CSS pixels, clamped.
    Size(u32),
    /// The user clicked a link to this absolute `http` or `https` URL.
    Open(String),
    /// The user pressed Escape in the page, and the page did not use it. A
    /// focused page gets every key, so the app learns of Escape this way.
    Escape,
}

/// The expression that resolves when the fonts are loaded and two frames
/// have been drawn.
pub const SETTLE_EXPRESSION: &str = "document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true)))))";

/// The expression that gives the height of the page in CSS pixels.
pub const MEASURE_EXPRESSION: &str = "(() => { const root = document.documentElement; return root.scrollHeight > root.clientHeight ? root.scrollHeight : root.getBoundingClientRect().height; })()";

/// The base sheet. It comes after the theme variables and before the CSS of
/// the page, so the page can override it.
const BASE_SHEET: &str = "html{box-sizing:border-box;padding:8px;background:var(--background);color:var(--foreground);font-family:var(--font-sans);font-size:14px;line-height:1.5;-webkit-font-smoothing:antialiased;-webkit-text-size-adjust:100%;scrollbar-width:none}html::-webkit-scrollbar{display:none}body{margin:0}code,kbd,pre,samp{font-family:var(--font-mono)}:where(:focus-visible){outline:2px solid var(--ring)}:focus-visible{outline-width:2px!important;outline-offset:-2px!important}";

/// The script of a webview. It reports the height of the page, sends the
/// links that the user clicks, and turns `window.open` off.
const BOOTSTRAP_SCRIPT: &str = r##"(function(){
function send(message){
  try{
    if(window.ipc&&typeof window.ipc.postMessage==="function"){
      window.ipc.postMessage(JSON.stringify(message));
    }
  }catch(_){}
}
var last=-1;
function report(){
  var r=document.documentElement,v=Math.ceil(r.scrollHeight>r.clientHeight?r.scrollHeight:r.getBoundingClientRect().height);
  if(v===last){return;}
  last=v;
  send({type:"size",height:v});
}
var observer=typeof ResizeObserver==="function"?new ResizeObserver(report):null;
var watched=null;
if(observer){observer.observe(document.documentElement);}
function watchBody(){
  if(observer&&document.body&&watched!==document.body){
    watched=document.body;
    observer.observe(document.body);
  }
  report();
}
if(document.readyState==="loading"){
  document.addEventListener("DOMContentLoaded",watchBody);
}else{
  watchBody();
}
window.addEventListener("load",report);
if(document.fonts&&document.fonts.ready){document.fonts.ready.then(report);}
document.addEventListener("click",function(event){
  var target=event.target;
  var link=target&&typeof target.closest==="function"?target.closest("a[href],area[href]"):null;
  if(!link){return;}
  var href=link.getAttribute("href")||"";
  var url=null;
  try{url=new URL(href,document.baseURI);}catch(_){}
  var same=url!==null&&url.href.split("#")[0]===document.URL.split("#")[0];
  if(href.charAt(0)==="#"||(same&&url.hash!=="")){return;}
  event.preventDefault();
  if(event.isTrusted&&url!==null&&!same&&(url.protocol==="http:"||url.protocol==="https:")){
    send({type:"open",url:url.href});
  }
},true);
window.open=function(){return null;};
window.addEventListener("keydown",function(event){
  if(event.key==="Escape"&&!event.defaultPrevented){send({type:"key",key:"Escape"});}
});
})();"##;

/// The ink kit: the hand-drawn chart library of the page.
const INK_SCRIPT: &str = include_str!("../kit/ink.js");

/// An opening `<head>` tag, with or without attributes. It does not match
/// `<header>`.
static HEAD_OPEN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?i)<head(?:\s(?:"[^"]*"|'[^']*'|[^>"'])*)?>"#).ok());

/// An opening `<html>` tag, with or without attributes.
static HTML_OPEN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?i)<html(?:\s(?:"[^"]*"|'[^']*'|[^>"'])*)?>"#).ok());

/// The faces of the app fonts: file name, family, style and weight. The
/// bytes come from `consort_theme::fonts::FILES`.
const FACES: [(&str, &str, &str, u16); 8] = [
    (
        "AtkinsonHyperlegibleNext-Regular.ttf",
        "Atkinson Hyperlegible Next",
        "normal",
        400,
    ),
    (
        "AtkinsonHyperlegibleNext-Italic.ttf",
        "Atkinson Hyperlegible Next",
        "italic",
        400,
    ),
    (
        "AtkinsonHyperlegibleNext-Medium.ttf",
        "Atkinson Hyperlegible Next",
        "normal",
        500,
    ),
    (
        "AtkinsonHyperlegibleNext-SemiBold.ttf",
        "Atkinson Hyperlegible Next",
        "normal",
        600,
    ),
    (
        "AtkinsonHyperlegibleNext-Bold.ttf",
        "Atkinson Hyperlegible Next",
        "normal",
        700,
    ),
    (
        "AtkinsonHyperlegibleMono-Regular.ttf",
        "Atkinson Hyperlegible Mono",
        "normal",
        400,
    ),
    (
        "AtkinsonHyperlegibleMono-Medium.ttf",
        "Atkinson Hyperlegible Mono",
        "normal",
        500,
    ),
    (
        "AtkinsonHyperlegibleMono-SemiBold.ttf",
        "Atkinson Hyperlegible Mono",
        "normal",
        600,
    ),
];

/// The `@font-face` rules of the app fonts, with the files as base64 data
/// URIs. Atkinson Hyperlegible Next has 400, 400 italic, 500, 600 and 700.
/// Atkinson Hyperlegible Mono has 400, 500 and 600. The text is built once.
pub fn fonts_css() -> &'static str {
    static CSS: LazyLock<String> = LazyLock::new(|| {
        let mut css = String::new();
        for (file, family, style, weight) in FACES {
            let Some((_, bytes)) = fonts::FILES.iter().find(|(name, _)| *name == file) else {
                continue;
            };
            css.push_str(&format!(
                "@font-face{{font-family:\"{family}\";font-style:{style};font-weight:{weight};font-display:block;src:url(\"data:font/ttf;base64,{}\") format(\"truetype\")}}",
                STANDARD.encode(bytes)
            ));
        }
        css
    });
    CSS.as_str()
}

/// Builds the document for a page: the bootstrap, then the page.
///
/// The bootstrap is the first child of `<head>`, so the CSS of the page
/// comes after it and can override it. It has the `<meta>` tags, the theme
/// variables, the fonts and the base sheet, for a webview the size reporter
/// and the link catcher, and the ink kit. The page is not changed or
/// escaped.
///
/// - A page with a `<head>` tag gets the bootstrap right after the tag.
/// - A page with an `<html>` tag but no `<head>` gets a new `<head>` right
///   after the `<html>` tag.
/// - Any other page is wrapped in a new document. A doctype at the start of
///   the page stays at the start.
///
/// A style that [`VisualStyle::check`] refuses gives [`PageError::Style`].
pub fn document(page: &str, style: &VisualStyle, host: Host) -> Result<String, PageError> {
    style.check()?;
    let bootstrap = bootstrap(style, host);
    if let Some(head) = HEAD_OPEN.as_ref().and_then(|re| re.find(page)) {
        return Ok(splice(page, head.end(), &bootstrap));
    }
    if let Some(html) = HTML_OPEN.as_ref().and_then(|re| re.find(page)) {
        return Ok(splice(
            page,
            html.end(),
            &format!("<head>{bootstrap}</head>"),
        ));
    }
    let start = page.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let (doctype, content) = match start.get(..9) {
        Some(head) if head.eq_ignore_ascii_case("<!doctype") => match start.find('>') {
            Some(end) => (&start[..=end], &start[end + 1..]),
            None => ("<!doctype html>", page),
        },
        _ => ("<!doctype html>", page),
    };
    Ok(format!(
        "{doctype}<html><head>{bootstrap}</head><body>{content}</body></html>"
    ))
}

/// `page` with `insert` put in at the byte `at`.
fn splice(page: &str, at: usize, insert: &str) -> String {
    let mut out = String::with_capacity(page.len() + insert.len());
    out.push_str(&page[..at]);
    out.push_str(insert);
    out.push_str(&page[at..]);
    out
}

/// The part that goes in front of the page.
fn bootstrap(style: &VisualStyle, host: Host) -> String {
    let scheme = style.mode.as_str();
    let fonts = fonts_css();
    let mut out = String::with_capacity(
        fonts.len() + BASE_SHEET.len() + BOOTSTRAP_SCRIPT.len() + INK_SCRIPT.len() + 2048,
    );
    out.push_str("<meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"");
    out.push_str(scheme);
    out.push_str("\"><style id=\"consort-visual-theme\">:root{color-scheme:");
    out.push_str(scheme);
    out.push(';');
    for variable in &style.variables {
        out.push_str(&variable.name);
        out.push(':');
        out.push_str(&variable.value);
        out.push(';');
    }
    out.push('}');
    out.push_str(fonts);
    out.push_str(BASE_SHEET);
    out.push_str("</style>");
    if host == Host::Webview {
        out.push_str("<script id=\"consort-visual-bootstrap\">");
        out.push_str(BOOTSTRAP_SCRIPT);
        out.push_str("</script>");
    }
    out.push_str("<script id=\"consort-visual-ink\">");
    out.push_str(INK_SCRIPT);
    out.push_str("</script>");
    out
}

/// The most bytes of a message from a page. The page can send any text, so
/// the parser reads no more than this.
const MESSAGE_MAX_BYTES: usize = 8 * 1024;
/// The most characters of the URL of an open message.
const URL_MAX_CHARS: usize = 2048;

/// Reads a message that the bootstrap sent with `window.ipc.postMessage`.
///
/// - `{"type":"size","height":n}` gives [`PageMessage::Size`] with the height
///   clamped by `clamp_height`.
/// - `{"type":"open","url":u}` gives [`PageMessage::Open`] when `u` is an
///   absolute `http` or `https` URL of at most 2048 characters. The URL must
///   be ASCII with no space, as `new URL(...).href` writes it. It must have a
///   host and no user name.
///
/// Anything else gives `None`.
pub fn parse_message(text: &str) -> Option<PageMessage> {
    if text.len() > MESSAGE_MAX_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match value.get("type")?.as_str()? {
        "size" => {
            let height = value.get("height")?.as_f64()?;
            Some(PageMessage::Size(clamp_height(height)))
        }
        "open" => {
            let url = value.get("url")?.as_str()?;
            is_web_url(url).then(|| PageMessage::Open(url.to_owned()))
        }
        "key" => (value.get("key")?.as_str()? == "Escape").then_some(PageMessage::Escape),
        _ => None,
    }
}

/// An absolute `http` or `https` URL in the form that `URL.href` writes.
fn is_web_url(url: &str) -> bool {
    if url.len() > URL_MAX_CHARS || !url.bytes().all(|b| b.is_ascii_graphic()) {
        return false;
    }
    let rest = if url
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
    {
        &url[8..]
    } else if url
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
    {
        &url[7..]
    } else {
        return false;
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    !authority.is_empty() && !authority.starts_with(':') && !authority.contains(['@', '\\'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{VARIABLES, default_style};
    use consort_proto::model::{CssVariable, VisualMode};

    fn dark() -> VisualStyle {
        default_style(VisualMode::Dark)
    }

    fn build(page: &str, host: Host) -> String {
        document(page, &dark(), host).expect("builds")
    }

    /// FNV-1a, 64 bit: a stable fingerprint with no dependency.
    fn fingerprint(parts: &[&[u8]]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for part in parts {
            for byte in *part {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
            hash ^= 0xff;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash
    }

    /// The fingerprint of the bootstrap for each `VISUAL_BOOTSTRAP_VERSION`.
    /// When this test fails, the bootstrap changed: add 1 to
    /// `VISUAL_BOOTSTRAP_VERSION` in consort-proto (so the hub draws new
    /// images), and put the new version and fingerprint here.
    const FINGERPRINT: (u32, u64) = (4, 0x9b19_569a_b42b_372f);

    #[test]
    fn a_change_to_the_bootstrap_changes_its_version() {
        let fonts: Vec<u8> = fonts::FILES
            .iter()
            .flat_map(|(_, bytes)| (bytes.len() as u64).to_le_bytes())
            .collect();
        let now = fingerprint(&[
            BASE_SHEET.as_bytes(),
            BOOTSTRAP_SCRIPT.as_bytes(),
            INK_SCRIPT.as_bytes(),
            &fonts,
        ]);
        assert_eq!(
            (consort_proto::model::VISUAL_BOOTSTRAP_VERSION, now),
            FINGERPRINT,
            "The bootstrap changed. Add 1 to VISUAL_BOOTSTRAP_VERSION and set FINGERPRINT to ({}, {now:#x}).",
            consort_proto::model::VISUAL_BOOTSTRAP_VERSION + 1
        );
    }

    #[test]
    fn escape_from_the_page_is_a_message() {
        assert_eq!(
            parse_message(r#"{"type":"key","key":"Escape"}"#),
            Some(PageMessage::Escape)
        );
        assert_eq!(parse_message(r#"{"type":"key","key":"Enter"}"#), None);
        assert!(BOOTSTRAP_SCRIPT.contains(r#"send({type:"key",key:"Escape"})"#));
    }

    /// The text that the bootstrap adds in front of the page, up to the
    /// closing tag of the ink script.
    fn bootstrap_text(host: Host) -> String {
        bootstrap(&dark(), host)
    }

    #[test]
    fn the_bootstrap_is_the_first_child_of_a_head() {
        let boot = bootstrap_text(Host::Webview);
        let page = "<!doctype html><html><head><title>T</title><style>p{color:red}</style></head><body><p>x</p></body></html>";
        let doc = build(page, Host::Webview);
        let want = format!(
            "<!doctype html><html><head>{boot}<title>T</title><style>p{{color:red}}</style></head><body><p>x</p></body></html>"
        );
        assert_eq!(doc, want);
        // The page CSS comes after the base sheet, so it can override it.
        let base = doc.find(BASE_SHEET).expect("base sheet");
        let page_css = doc.find("p{color:red}").expect("page css");
        assert!(base < page_css);
    }

    #[test]
    fn a_head_tag_with_attributes_and_capitals_is_found() {
        let boot = bootstrap_text(Host::Webview);
        let doc = build(
            "<HTML><HEAD lang=x data-a=\"1>2\"><title>T</title></HEAD><BODY>b</BODY></HTML>",
            Host::Webview,
        );
        assert_eq!(
            doc,
            format!(
                "<HTML><HEAD lang=x data-a=\"1>2\">{boot}<title>T</title></HEAD><BODY>b</BODY></HTML>"
            )
        );
    }

    #[test]
    fn a_page_with_only_html_gets_a_new_head() {
        let boot = bootstrap_text(Host::Webview);
        let doc = build(
            "<html lang=\"en\"><body><p>x</p></body></html>",
            Host::Webview,
        );
        assert_eq!(
            doc,
            format!("<html lang=\"en\"><head>{boot}</head><body><p>x</p></body></html>")
        );
    }

    #[test]
    fn a_fragment_is_wrapped() {
        let boot = bootstrap_text(Host::Webview);
        let doc = build("<div>hi</div>", Host::Webview);
        assert_eq!(
            doc,
            format!("<!doctype html><html><head>{boot}</head><body><div>hi</div></body></html>")
        );
    }

    #[test]
    fn a_doctype_of_the_page_stays_at_the_very_start() {
        let boot = bootstrap_text(Host::Webview);
        let doc = build(
            "\n  <!DOCTYPE html PUBLIC \"x\">\n<div>hi</div>",
            Host::Webview,
        );
        assert_eq!(
            doc,
            format!(
                "<!DOCTYPE html PUBLIC \"x\"><html><head>{boot}</head><body>\n<div>hi</div></body></html>"
            )
        );
        assert!(doc.starts_with("<!DOCTYPE"));
    }

    #[test]
    fn a_header_tag_is_not_a_head_tag() {
        let boot = bootstrap_text(Host::Webview);
        let page = "<header><h1>Title</h1></header><main>x</main>";
        let doc = build(page, Host::Webview);
        assert_eq!(
            doc,
            format!("<!doctype html><html><head>{boot}</head><body>{page}</body></html>")
        );
        // A `<header>` after an `<html>` tag does not take the bootstrap.
        let doc = build("<html><header>h</header></html>", Host::Webview);
        assert_eq!(
            doc,
            format!("<html><head>{boot}</head><header>h</header></html>")
        );
    }

    #[test]
    fn the_page_is_not_escaped_or_changed() {
        let page = "<p>5 < 6 & \"quotes\" </script></p>";
        let doc = build(page, Host::Renderer);
        assert!(doc.contains(page));
    }

    #[test]
    fn the_head_of_the_document_starts_with_the_meta_tags() {
        let doc = build("<div>x</div>", Host::Webview);
        assert!(doc.contains(
            "<head><meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"dark\"><style id=\"consort-visual-theme\">:root{color-scheme:dark;--background:#131E1A;"
        ));
        let light = document(
            "<div>x</div>",
            &default_style(VisualMode::Light),
            Host::Webview,
        )
        .expect("builds");
        assert!(light.contains("<meta name=\"color-scheme\" content=\"light\">"));
        assert!(light.contains(":root{color-scheme:light;--background:#F8FCF8;"));
    }

    #[test]
    fn the_theme_variables_come_in_order_then_the_fonts_then_the_base_sheet() {
        let doc = build("<div>x</div>", Host::Webview);
        let theme = doc
            .find("<style id=\"consort-visual-theme\">")
            .expect("style");
        let root_end = doc[theme..].find('}').expect("end of :root") + theme;
        let root = &doc[theme..root_end];
        let mut at = 0;
        for name in VARIABLES {
            let found = root[at..]
                .find(&format!("{name}:"))
                .unwrap_or_else(|| panic!("{name} is missing or out of order in {root}"));
            at += found + name.len();
        }
        let fonts = doc.find("@font-face").expect("fonts");
        let base = doc.find("html{box-sizing:border-box").expect("base");
        assert!(root_end < fonts && fonts < base);
        assert!(doc.contains(&format!("{BASE_SHEET}</style>")));
    }

    #[test]
    fn a_webview_gets_the_bootstrap_script_and_a_renderer_does_not() {
        let webview = build("<div>x</div>", Host::Webview);
        assert!(webview.contains("<script id=\"consort-visual-bootstrap\">"));
        assert!(webview.contains("window.ipc.postMessage"));
        let renderer = build("<div>x</div>", Host::Renderer);
        assert!(!renderer.contains("consort-visual-bootstrap"));
        assert!(!renderer.contains("window.ipc"));
        // Both get the theme and the ink kit.
        for doc in [&webview, &renderer] {
            assert!(doc.contains("<style id=\"consort-visual-theme\">"));
            assert!(doc.contains(&format!(
                "<script id=\"consort-visual-ink\">{INK_SCRIPT}</script>"
            )));
        }
        // The order in a webview: theme, bootstrap, ink.
        let theme = webview.find("consort-visual-theme").expect("theme");
        let boot = webview.find("consort-visual-bootstrap").expect("bootstrap");
        let ink = webview.find("consort-visual-ink").expect("ink");
        assert!(theme < boot && boot < ink);
    }

    #[test]
    fn the_scripts_cannot_end_their_own_element() {
        // A `</script` inside a script would cut the script in the page.
        assert!(!BOOTSTRAP_SCRIPT.to_ascii_lowercase().contains("</script"));
        assert!(!INK_SCRIPT.to_ascii_lowercase().contains("</script"));
        assert!(!fonts_css().to_ascii_lowercase().contains("</style"));
    }

    #[test]
    fn the_size_reporter_uses_the_documented_measure() {
        assert!(BOOTSTRAP_SCRIPT.contains(
            "Math.ceil(r.scrollHeight>r.clientHeight?r.scrollHeight:r.getBoundingClientRect().height)"
        ));
        assert!(BOOTSTRAP_SCRIPT.contains("new ResizeObserver(report)"));
        assert!(BOOTSTRAP_SCRIPT.contains("observer.observe(document.documentElement)"));
        assert!(BOOTSTRAP_SCRIPT.contains("observer.observe(document.body)"));
        assert!(BOOTSTRAP_SCRIPT.contains("window.addEventListener(\"load\",report)"));
        assert!(BOOTSTRAP_SCRIPT.contains("document.fonts.ready.then(report)"));
        assert!(BOOTSTRAP_SCRIPT.contains("JSON.stringify(message)"));
        assert!(BOOTSTRAP_SCRIPT.contains("{type:\"size\",height:v}"));
    }

    #[test]
    fn the_link_catcher_is_in_the_capture_phase_and_the_window_open_is_off() {
        assert!(BOOTSTRAP_SCRIPT.contains("document.addEventListener(\"click\""));
        assert!(BOOTSTRAP_SCRIPT.contains("},true);"));
        assert!(BOOTSTRAP_SCRIPT.contains("closest(\"a[href],area[href]\")"));
        assert!(BOOTSTRAP_SCRIPT.contains("event.preventDefault()"));
        assert!(BOOTSTRAP_SCRIPT.contains("event.isTrusted"));
        assert!(BOOTSTRAP_SCRIPT.contains("{type:\"open\",url:url.href}"));
        assert!(BOOTSTRAP_SCRIPT.contains("window.open=function(){return null;};"));
    }

    #[test]
    fn the_bootstrap_script_is_balanced() {
        let count = |c: char| BOOTSTRAP_SCRIPT.chars().filter(|x| *x == c).count();
        assert_eq!(count('{'), count('}'));
        assert_eq!(count('('), count(')'));
        assert_eq!(count('['), count(']'));
    }

    #[test]
    fn a_style_that_is_not_allowed_is_refused_first() {
        let style = VisualStyle {
            mode: VisualMode::Dark,
            variables: vec![CssVariable {
                name: "--background".to_owned(),
                value: "red;}</style><script>alert(1)</script>".to_owned(),
            }],
        };
        let err = document("<p>x</p>", &style, Host::Webview).expect_err("refused");
        assert!(matches!(err, PageError::Style(_)), "{err}");
    }

    #[test]
    fn the_fonts_have_eight_faces_with_data_uris() {
        let css = fonts_css();
        assert_eq!(css.matches("@font-face").count(), 8);
        assert_eq!(css.matches("font-display:block").count(), 8);
        assert_eq!(css.matches("format(\"truetype\")").count(), 8);
        assert_eq!(css.matches("src:url(\"data:font/ttf;base64,").count(), 8);
        for weight in [400, 500, 600, 700] {
            assert!(css.contains(&format!(
                "font-family:\"Atkinson Hyperlegible Next\";font-style:normal;font-weight:{weight};"
            )));
        }
        assert!(css.contains(
            "font-family:\"Atkinson Hyperlegible Next\";font-style:italic;font-weight:400;"
        ));
        for weight in [400, 500, 600] {
            assert!(css.contains(&format!(
                "font-family:\"Atkinson Hyperlegible Mono\";font-style:normal;font-weight:{weight};"
            )));
        }
        // The data of the first file is in the rule.
        let first = STANDARD.encode(fonts::FILES[0].1);
        assert!(css.contains(&first));
        // It is built once.
        assert!(std::ptr::eq(fonts_css(), fonts_css()));
    }

    #[test]
    fn the_faces_name_every_font_file_once() {
        for (name, _) in fonts::FILES {
            assert_eq!(
                FACES.iter().filter(|(file, ..)| *file == name).count(),
                1,
                "{name}"
            );
        }
    }

    #[test]
    fn size_messages_are_clamped() {
        assert_eq!(
            parse_message(r#"{"type":"size","height":321}"#),
            Some(PageMessage::Size(321))
        );
        assert_eq!(
            parse_message(r#"{"type":"size","height":200.2}"#),
            Some(PageMessage::Size(201))
        );
        assert_eq!(
            parse_message(r#"{"type":"size","height":5}"#),
            Some(PageMessage::Size(80))
        );
        assert_eq!(
            parse_message(r#"{"type":"size","height":-40}"#),
            Some(PageMessage::Size(80))
        );
        assert_eq!(
            parse_message(r#"{"type":"size","height":1e12}"#),
            Some(PageMessage::Size(20_000))
        );
        assert_eq!(parse_message(r#"{"type":"size"}"#), None);
        assert_eq!(parse_message(r#"{"type":"size","height":"300"}"#), None);
        assert_eq!(parse_message(r#"{"type":"size","height":null}"#), None);
    }

    #[test]
    fn open_messages_need_an_absolute_web_url() {
        let open =
            |url: &str| parse_message(&serde_json::json!({"type": "open", "url": url}).to_string());
        assert_eq!(
            open("https://example.com/a?b=1#c"),
            Some(PageMessage::Open("https://example.com/a?b=1#c".to_owned()))
        );
        assert_eq!(
            open("http://localhost:8080/"),
            Some(PageMessage::Open("http://localhost:8080/".to_owned()))
        );
        assert_eq!(
            open("HTTPS://Example.com"),
            Some(PageMessage::Open("HTTPS://Example.com".to_owned()))
        );
        for bad in [
            "",
            "example.com",
            "//example.com/a",
            "/a/b",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "ftp://example.com/",
            "mailto:a@b.c",
            "https://",
            "https:///a",
            "https://user:pw@example.com/",
            "https://example.com@evil.example/",
            "https://exa mple.com/",
            "https://example.com/a b",
            "https://example.com/\n",
            "https://exämple.com/",
            "https:\\\\example.com",
            "https://example.com\\@evil.example/",
        ] {
            assert_eq!(open(bad), None, "{bad:?}");
        }
        let at_limit = format!("https://example.com/{}", "a".repeat(2048 - 20));
        assert_eq!(at_limit.len(), 2048);
        assert_eq!(open(&at_limit), Some(PageMessage::Open(at_limit.clone())));
        assert_eq!(open(&format!("{at_limit}a")), None);
    }

    #[test]
    fn other_messages_are_ignored() {
        for text in [
            "",
            "not json",
            "[]",
            "null",
            "42",
            r#"{"type":"close"}"#,
            r#"{"type":5}"#,
            r#"{"height":300}"#,
            r#"{"type":"open"}"#,
            r#"{"type":"open","url":7}"#,
        ] {
            assert_eq!(parse_message(text), None, "{text}");
        }
        let big = format!(
            r#"{{"type":"size","height":300,"x":"{}"}}"#,
            "a".repeat(9000)
        );
        assert_eq!(parse_message(&big), None);
    }

    #[test]
    fn the_expressions_are_the_documented_ones() {
        assert_eq!(
            SETTLE_EXPRESSION,
            "document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true)))))"
        );
        assert_eq!(
            MEASURE_EXPRESSION,
            "(() => { const root = document.documentElement; return root.scrollHeight > root.clientHeight ? root.scrollHeight : root.getBoundingClientRect().height; })()"
        );
    }
}
