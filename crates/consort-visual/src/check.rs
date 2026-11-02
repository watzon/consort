//! The page check: the size of the page and the viewport height units.

use std::sync::LazyLock;

use consort_proto::model::VISUAL_HTML_MAX_CHARS;
use regex::Regex;

use crate::{
    error::PageError,
    scan::{style_blocks, tags},
};

/// The most samples that [`PageError::ViewportUnits`] carries.
const SAMPLES_MAX: usize = 5;

/// A viewport height unit with its number, such as `100vh` or `.5svh`. This
/// is the pattern of the T3 Code check, with no look-around. The text before
/// and after the match is checked by hand in [`viewport_samples`].
static VIEWPORT_HEIGHT: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)\A(?:\d+(?:\.\d+)?|\.\d+)[sld]?vh").ok());

/// Checks the HTML that the assistant wrote.
///
/// - Only white space gives [`PageError::Empty`].
/// - More than `VISUAL_HTML_MAX_CHARS` characters gives
///   [`PageError::TooLong`].
/// - A viewport height unit (`vh`, `svh`, `lvh`, `dvh`) in a `<style>` block
///   or in a `style=` attribute gives [`PageError::ViewportUnits`]. A visual
///   grows to fit its page, so a page sized to the viewport would grow again
///   and again. The same units in text are allowed.
pub fn check_html(html: &str) -> Result<(), PageError> {
    if html.trim().is_empty() {
        return Err(PageError::Empty);
    }
    let chars = html.chars().count();
    if chars > VISUAL_HTML_MAX_CHARS {
        return Err(PageError::TooLong { chars });
    }
    let found = viewport_height_units(html);
    if !found.is_empty() {
        return Err(PageError::ViewportUnits { found });
    }
    Ok(())
}

/// Up to five different viewport height units from the `<style>` blocks and
/// the `style=` attributes, in the order of the page.
fn viewport_height_units(html: &str) -> Vec<String> {
    let blocks = style_blocks(html);
    let mut found = Vec::new();
    for block in &blocks {
        viewport_samples(&html[block.clone()], &mut found);
    }
    for tag in tags(html, &blocks) {
        for attribute in tag.attributes.iter().filter(|a| a.name == "style") {
            viewport_samples(&html[attribute.value.clone()], &mut found);
        }
    }
    found
}

/// Adds the viewport height units in `css` to `found`, up to the limit.
///
/// The rule is `(?<![\w.+/])NUMBER[sld]?vh(?![\w-])`. A unit is not a unit
/// when a word character, `.`, `+` or `/` comes before the number, or when a
/// word character or `-` comes after `vh`.
fn viewport_samples(css: &str, found: &mut Vec<String>) {
    let Some(re) = VIEWPORT_HEIGHT.as_ref() else {
        return;
    };
    for (start, c) in css.char_indices() {
        if found.len() >= SAMPLES_MAX {
            return;
        }
        if !(c.is_ascii_digit() || c == '.') {
            continue;
        }
        let before = css[..start].chars().next_back();
        if before.is_some_and(|b| is_word(b) || matches!(b, '.' | '+' | '/')) {
            continue;
        }
        let Some(m) = re.find(&css[start..]) else {
            continue;
        };
        let end = start + m.end();
        let after = css[end..].chars().next();
        if after.is_some_and(|a| is_word(a) || a == '-') {
            continue;
        }
        let sample = m.as_str().to_owned();
        if !found.contains(&sample) {
            found.push(sample);
        }
    }
}

/// A word character, as in `\w`.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(html: &str) -> Vec<String> {
        match check_html(html) {
            Err(PageError::ViewportUnits { found }) => found,
            Ok(()) => Vec::new(),
            Err(other) => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn white_space_only_is_empty() {
        assert_eq!(check_html(""), Err(PageError::Empty));
        assert_eq!(check_html(" \n\t  "), Err(PageError::Empty));
        assert_eq!(check_html("<p>x</p>"), Ok(()));
    }

    #[test]
    fn a_long_page_is_refused_at_the_limit() {
        let at_limit = "a".repeat(VISUAL_HTML_MAX_CHARS);
        assert_eq!(check_html(&at_limit), Ok(()));
        let over = "a".repeat(VISUAL_HTML_MAX_CHARS + 1);
        assert_eq!(
            check_html(&over),
            Err(PageError::TooLong {
                chars: VISUAL_HTML_MAX_CHARS + 1
            })
        );
        // The count is in characters, not bytes.
        let wide = "é".repeat(VISUAL_HTML_MAX_CHARS);
        assert_eq!(check_html(&wide), Ok(()));
    }

    #[test]
    fn viewport_height_units_in_a_style_block_are_found() {
        for (css, want) in [
            ("body{min-height:100vh}", "100vh"),
            ("body{height:50.5dvh}", "50.5dvh"),
            ("body{height:.5svh}", ".5svh"),
            ("body{height:30lvh}", "30lvh"),
            ("body{height:100VH}", "100VH"),
            ("body{height:calc(100vh - 20px)}", "100vh"),
            ("body{margin-top:-100vh}", "100vh"),
        ] {
            let html = format!("<style>{css}</style><p>x</p>");
            assert_eq!(units(&html), [want], "{css}");
        }
    }

    #[test]
    fn viewport_height_units_in_a_style_attribute_are_found() {
        for (tag, want) in [
            (r#"<div style="height:100vh">"#, "100vh"),
            (r#"<div style='height:50.5dvh'>"#, "50.5dvh"),
            (r#"<div style="height:.5svh">"#, ".5svh"),
            (r#"<div style=height:30lvh>"#, "30lvh"),
            (r#"<div class="a" STYLE = "height: 1vh;">"#, "1vh"),
        ] {
            assert_eq!(units(tag), [want], "{tag}");
        }
    }

    #[test]
    fn text_that_looks_like_a_unit_is_not_a_unit() {
        for css in [
            "body{height:100vhx}",
            "body{height:100vh-x}",
            "body{height:x100vh}",
            "body{height:a100vh}",
            "body{width:100vw}",
            "body{width:1.5.5vh}",
            "body{background:url(/img/100vh)}",
            "body{height:+100vh}",
        ] {
            assert_eq!(
                units(&format!("<style>{css}</style>")),
                Vec::<String>::new(),
                "{css}"
            );
        }
    }

    #[test]
    fn units_outside_styles_are_allowed() {
        assert_eq!(check_html("<p>Use 100vh for the height.</p>"), Ok(()));
        assert_eq!(check_html("<p>The class a-100vh sets it.</p>"), Ok(()));
        assert_eq!(
            check_html(r#"<div class="h-100vh" title="100vh">x</div>"#),
            Ok(())
        );
        assert_eq!(
            check_html("<script>el.style.height = '100vh'</script>"),
            Ok(())
        );
        // A style attribute in the text of a code sample is not a tag.
        assert_eq!(
            check_html(r#"<p>Write &lt;div style="height:100vh"&gt;</p>"#),
            Ok(())
        );
    }

    #[test]
    fn the_message_names_up_to_five_different_samples() {
        let css = "a{height:1vh}b{height:2vh}c{height:3vh}d{height:4vh}e{height:5vh}f{height:6vh}g{height:1vh}";
        let found = units(&format!("<style>{css}</style>"));
        assert_eq!(found, ["1vh", "2vh", "3vh", "4vh", "5vh"]);
        let message = check_html(&format!("<style>{css}</style>"))
            .expect_err("refused")
            .to_string();
        assert!(message.contains("viewport height units (1vh, 2vh, 3vh, 4vh, 5vh)"));
        assert!(message.contains("Set heights in px"));
    }

    #[test]
    fn style_blocks_and_attributes_are_both_scanned_in_page_order() {
        let html = r#"<div style="height:10vh"></div><style>a{height:20vh}</style>"#;
        // The blocks come first, then the attributes.
        let mut found = units(html);
        found.sort();
        assert_eq!(found, ["10vh", "20vh"]);
    }

    #[test]
    fn script_code_does_not_hide_a_style_attribute_after_it() {
        let html = "<script>for(var i=0;i<n;i++){} // don't\n</script><div style='height:100vh'>";
        assert_eq!(units(html), ["100vh"]);
    }

    #[test]
    fn an_unterminated_style_block_is_scanned_to_the_end() {
        assert_eq!(units("<style>a{height:100vh}"), ["100vh"]);
    }
}
