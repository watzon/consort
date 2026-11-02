//! A small scanner for the tags, the `<style>` blocks and the attributes of
//! a page. It is not an HTML parser. It finds the places that the page check
//! and the image inlining need, and it gives the byte span of each value.

use std::{ops::Range, sync::LazyLock};

use regex::Regex;

/// One attribute of a tag: its lowercase name and the span of its value in
/// the page. The span is that of the text between the quotes.
#[derive(Debug, Clone)]
pub(crate) struct Attribute {
    pub name: String,
    pub value: Range<usize>,
}

/// One opening tag: its lowercase name and its attributes.
#[derive(Debug, Clone)]
pub(crate) struct Tag {
    pub name: String,
    pub attributes: Vec<Attribute>,
}

/// An opening tag. Quoted values may hold `>`.
static TAG: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?s)<([A-Za-z][^\s/>]*)((?:"[^"]*"|'[^']*'|[^>"'])*)>"#).ok());

/// One attribute in the part of a tag after its name.
static ATTRIBUTE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r#"([^\s"'<>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?"#).ok()
});

/// A `<style>` block. An unterminated block runs to the end of the page, as
/// it does in a browser.
static STYLE_BLOCK: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?is)<style\b[^>]*>(.*?)(?:</style\s*>|\z)").ok());

/// A `<script>` block, with the same rule for an unterminated block.
static SCRIPT_BLOCK: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?is)<script\b[^>]*>(.*?)(?:</script\s*>|\z)").ok());

/// The spans of the text inside the `<style>` blocks.
pub(crate) fn style_blocks(html: &str) -> Vec<Range<usize>> {
    blocks(&STYLE_BLOCK, html)
}

/// The spans of the text inside the `<script>` blocks.
fn script_blocks(html: &str) -> Vec<Range<usize>> {
    blocks(&SCRIPT_BLOCK, html)
}

fn blocks(re: &LazyLock<Option<Regex>>, html: &str) -> Vec<Range<usize>> {
    let Some(re) = re.as_ref() else {
        return Vec::new();
    };
    re.captures_iter(html)
        .filter_map(|caps| caps.get(1))
        .map(|m| m.range())
        .collect()
}

/// The parts of the page that are not inside a `<style>` or a `<script>`
/// block. Code can hold `<`, quotes and `>` in any order, so the tag scan
/// must not run over it. `styles` comes from [`style_blocks`].
fn markup_segments(html: &str, styles: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut skip: Vec<Range<usize>> = styles.to_vec();
    skip.extend(script_blocks(html));
    skip.sort_by_key(|r| r.start);
    let mut segments = Vec::new();
    let mut at = 0;
    for block in skip {
        if block.start > at {
            segments.push(at..block.start);
        }
        at = at.max(block.end);
    }
    if at < html.len() {
        segments.push(at..html.len());
    }
    segments
}

/// Every opening tag of the page, outside the `<style>` and `<script>`
/// blocks, with its attributes. `styles` comes from [`style_blocks`].
pub(crate) fn tags(html: &str, styles: &[Range<usize>]) -> Vec<Tag> {
    let (Some(tag_re), Some(attr_re)) = (TAG.as_ref(), ATTRIBUTE.as_ref()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for segment in markup_segments(html, styles) {
        for caps in tag_re.captures_iter(&html[segment.clone()]) {
            let (Some(name), Some(rest)) = (caps.get(1), caps.get(2)) else {
                continue;
            };
            let rest_start = segment.start + rest.start();
            let mut attributes = Vec::new();
            for attr in attr_re.captures_iter(rest.as_str()) {
                let Some(attr_name) = attr.get(1) else {
                    continue;
                };
                let value = attr
                    .get(2)
                    .or_else(|| attr.get(3))
                    .or_else(|| attr.get(4))
                    .map(|m| rest_start + m.start()..rest_start + m.end())
                    .unwrap_or(0..0);
                attributes.push(Attribute {
                    name: attr_name.as_str().to_ascii_lowercase(),
                    value,
                });
            }
            out.push(Tag {
                name: name.as_str().to_ascii_lowercase(),
                attributes,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(html: &str) -> Vec<String> {
        tags(html, &style_blocks(html))
            .into_iter()
            .map(|t| t.name)
            .collect()
    }

    #[test]
    fn code_in_a_script_does_not_swallow_the_tags_after_it() {
        let html = "<script>for(var i=0;i<n;i++){x='a'} // don't\nvar s = '<img src=\"/in-script.png\">';</script><img src=\"/x.png\"><p style='k:v'>";
        assert_eq!(names(html), ["script", "img", "p"]);
    }

    #[test]
    fn code_in_a_style_block_is_skipped() {
        let html = "<style>a<b{content:'<i>'}</style><b>";
        assert_eq!(names(html), ["style", "b"]);
    }

    #[test]
    fn an_unterminated_script_runs_to_the_end() {
        assert_eq!(names("<p><script>if(a<b){}<img src=x>"), ["p", "script"]);
    }

    #[test]
    fn the_value_spans_point_into_the_page() {
        let html = "<script>var a=1;</script><img alt=\"x\" src='/a.png' hidden>";
        let all = tags(html, &style_blocks(html));
        let img = all.iter().find(|t| t.name == "img").expect("img");
        let src = img
            .attributes
            .iter()
            .find(|a| a.name == "src")
            .expect("src");
        assert_eq!(&html[src.value.clone()], "/a.png");
        let alt = img
            .attributes
            .iter()
            .find(|a| a.name == "alt")
            .expect("alt");
        assert_eq!(&html[alt.value.clone()], "x");
        let hidden = img
            .attributes
            .iter()
            .find(|a| a.name == "hidden")
            .expect("hidden");
        assert!(hidden.value.is_empty());
    }
}
