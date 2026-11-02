//! Local images: find the references in a page, check the files by their
//! magic bytes, and inline them as `data:` URIs. Nothing here reads a file.
//! The session host reads the files through the worker.

use std::{ops::Range, sync::LazyLock};

use base64::{Engine, engine::general_purpose::STANDARD};
use consort_proto::model::{VISUAL_IMAGE_MAX_BYTES, VISUAL_PAGE_MAX_BYTES};
use regex::Regex;

use crate::{
    error::PageError,
    scan::{style_blocks, tags},
};

/// A reference to a local image in a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalImage {
    /// The reference as the page writes it, such as `/tmp/chart.png` or
    /// `file:///tmp/my%20chart.png`.
    pub reference: String,
    /// The path of the file: the reference with the `file://` form and the
    /// `%XX` escapes decoded, such as `/tmp/my chart.png`.
    pub path: String,
}

/// One use of a local image: where it is in the page, and what it is.
struct Use {
    span: Range<usize>,
    image: LocalImage,
}

/// `url(...)` in CSS.
static CSS_URL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?i)url\(\s*(?:"([^"]*)"|'([^']*)'|([^)"'\s]*))\s*\)"#).ok());

/// The tags that take a `src` attribute with an image, a video or a sound.
const SRC_TAGS: [&str; 6] = ["img", "source", "image", "input", "video", "audio"];

/// The local images that a page uses, once each, in the order of the page.
///
/// The references are in `src=` of `img`, `source`, `image`, `input`,
/// `video` and `audio`, in `poster=` of `video`, in `href=` and
/// `xlink:href=` of an SVG `image`, and in `url(...)` of the `<style>`
/// blocks and the `style=` attributes. A reference is local when it is one
/// of these:
///
/// - an absolute POSIX path, such as `/tmp/a.png`, but not `//host/a.png`;
/// - a `file://` URL, with `%XX` escapes. The host may be empty or
///   `localhost`;
/// - a Windows path, such as `C:\a.png`, `C:/a.png` or `file:///C:/a.png`.
///
/// `http:`, `https:`, `data:` and `blob:` URLs and relative paths are not
/// local.
pub fn local_images(html: &str) -> Vec<LocalImage> {
    let mut seen: Vec<LocalImage> = Vec::new();
    for item in uses(html) {
        if !seen.iter().any(|s| s.reference == item.image.reference) {
            seen.push(item.image);
        }
    }
    seen
}

/// Every use of a local image in the page, in the order of the page.
fn uses(html: &str) -> Vec<Use> {
    let blocks = style_blocks(html);
    let mut out: Vec<Use> = Vec::new();
    for block in &blocks {
        css_uses(html, block.clone(), &mut out);
    }
    for tag in tags(html, &blocks) {
        for attribute in &tag.attributes {
            let name = attribute.name.as_str();
            let is_reference = match name {
                "src" => SRC_TAGS.contains(&tag.name.as_str()),
                "poster" => tag.name == "video",
                "href" | "xlink:href" => tag.name == "image",
                _ => false,
            };
            if is_reference {
                if let Some(item) = use_of(html, attribute.value.clone()) {
                    out.push(item);
                }
            } else if name == "style" {
                css_uses(html, attribute.value.clone(), &mut out);
            }
        }
    }
    out.sort_by_key(|item| item.span.start);
    out
}

/// Adds the local images in `url(...)` forms inside `span` of the page.
fn css_uses(html: &str, span: Range<usize>, out: &mut Vec<Use>) {
    let Some(re) = CSS_URL.as_ref() else {
        return;
    };
    let base = span.start;
    for caps in re.captures_iter(&html[span]) {
        let Some(m) = caps.get(1).or_else(|| caps.get(2)).or_else(|| caps.get(3)) else {
            continue;
        };
        if let Some(item) = use_of(html, base + m.start()..base + m.end()) {
            out.push(item);
        }
    }
}

/// The quote forms that an entity can write inside a `style=` attribute.
const ENTITY_QUOTES: [&str; 4] = ["&quot;", "&#34;", "&#39;", "&apos;"];

/// The use at `span`, if its text is a local reference. The span shrinks to
/// leave out white space and entity quotes.
fn use_of(html: &str, span: Range<usize>) -> Option<Use> {
    let mut text = &html[span.clone()];
    let mut start = span.start;
    loop {
        let trimmed = text.trim_start();
        start += text.len() - trimmed.len();
        text = trimmed;
        match ENTITY_QUOTES.iter().find(|q| text.starts_with(**q)) {
            Some(quote) => {
                text = &text[quote.len()..];
                start += quote.len();
            }
            None => break,
        }
    }
    loop {
        text = text.trim_end();
        match ENTITY_QUOTES.iter().find(|q| text.ends_with(**q)) {
            Some(quote) => text = &text[..text.len() - quote.len()],
            None => break,
        }
    }
    let path = local_path(text)?;
    Some(Use {
        span: start..start + text.len(),
        image: LocalImage {
            reference: text.to_owned(),
            path,
        },
    })
}

/// The file path of a local reference, or `None` for any other reference.
fn local_path(reference: &str) -> Option<String> {
    let reference = reference.replace("&amp;", "&");
    let lower = reference.to_ascii_lowercase();
    if lower.starts_with("file:") {
        return file_url_path(&reference);
    }
    if is_windows_path(&reference) {
        return Some(reference);
    }
    // `//host/path` is a URL with no scheme, not a local file. A single `/`
    // names no file.
    if reference.len() > 1 && reference.starts_with('/') && !reference.starts_with("//") {
        return Some(reference);
    }
    None
}

/// `C:\x` or `C:/x`.
fn is_windows_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// The path in `file:///x`, `file://localhost/x` and `file:///C:/x`, with the
/// `%XX` escapes decoded. The query and the fragment are not part of it.
fn file_url_path(url: &str) -> Option<String> {
    let rest = url.get("file:".len()..)?;
    let path = match rest.strip_prefix("//") {
        Some(after) => {
            let slash = after.find('/')?;
            let (host, path) = after.split_at(slash);
            if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
                return None;
            }
            path
        }
        // `file:/x` has no host part.
        None if rest.starts_with('/') => rest,
        None => return None,
    };
    let end = path.find(['?', '#']).unwrap_or(path.len());
    let decoded = percent_decode(&path[..end])?;
    // `/C:/x` is a Windows path.
    let windows = decoded.strip_prefix('/').filter(|p| is_windows_path(p));
    let path = windows.map_or(decoded.as_str(), |p| p).to_owned();
    (path.len() > 1).then_some(path)
}

/// Decodes `%XX`. `None` when the bytes are not UTF-8 or an escape is
/// broken.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The media type of an image, by its first bytes only.
///
/// - `image/png`, `image/jpeg`, `image/gif` and `image/webp` by their magic
///   bytes.
/// - `image/svg+xml` for UTF-8 text with no NUL byte that starts with `<svg`.
///   A BOM, white space, an `<?xml ...?>` declaration, comments and a doctype
///   may come before it.
///
/// The file name and the extension do not count, so a renamed file does not
/// pass.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    is_svg(bytes).then_some("image/svg+xml")
}

/// SVG text: UTF-8, no NUL byte, and `<svg` is the first element.
fn is_svg(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut rest = text.strip_prefix('\u{feff}').unwrap_or(text);
    loop {
        rest = rest.trim_start();
        if rest.starts_with("<?") {
            match rest.find("?>") {
                Some(end) => rest = &rest[end + 2..],
                None => return false,
            }
        } else if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => rest = &rest[end + 3..],
                None => return false,
            }
        } else if rest
            .get(..9)
            .is_some_and(|head| head.eq_ignore_ascii_case("<!doctype"))
        {
            match doctype_end(rest) {
                Some(end) => rest = &rest[end..],
                None => return false,
            }
        } else {
            break;
        }
    }
    rest.strip_prefix("<svg")
        .and_then(|after| after.chars().next())
        .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
}

/// The end of a doctype that starts at the beginning of `text`. A doctype may
/// have an internal subset in `[...]`.
fn doctype_end(text: &str) -> Option<usize> {
    let close = text.find('>')?;
    match text.find('[') {
        Some(open) if open < close => {
            let subset_end = text[open..].find(']')? + open;
            Some(text[subset_end..].find('>')? + subset_end + 1)
        }
        _ => Some(close + 1),
    }
}

/// Puts the files into the page. Each use of a reference in `files` becomes a
/// `data:<mime>;base64,<data>` URI. A reference with no file stays as it is.
///
/// - A file of more than `VISUAL_IMAGE_MAX_BYTES` gives
///   [`PageError::ImageTooLarge`].
/// - A file that is not an image gives [`PageError::NotAnImage`].
/// - A page of more than `VISUAL_PAGE_MAX_BYTES` bytes gives
///   [`PageError::PageTooLarge`].
pub fn inline_images(html: &str, files: &[(LocalImage, Vec<u8>)]) -> Result<String, PageError> {
    let mut data_uris: Vec<(&str, String)> = Vec::new();
    for (image, bytes) in files {
        if bytes.len() as u64 > VISUAL_IMAGE_MAX_BYTES {
            return Err(PageError::ImageTooLarge {
                path: image.path.clone(),
            });
        }
        let Some(mime) = sniff_image(bytes) else {
            return Err(PageError::NotAnImage {
                path: image.path.clone(),
            });
        };
        if data_uris.iter().any(|(r, _)| *r == image.reference) {
            continue;
        }
        data_uris.push((
            image.reference.as_str(),
            format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
        ));
    }
    let mut out = String::with_capacity(html.len());
    let mut copied = 0;
    for item in uses(html) {
        let Some((_, uri)) = data_uris.iter().find(|(r, _)| *r == item.image.reference) else {
            continue;
        };
        out.push_str(&html[copied..item.span.start]);
        out.push_str(uri);
        copied = item.span.end;
        if out.len() as u64 > VISUAL_PAGE_MAX_BYTES {
            return Err(PageError::PageTooLarge {
                bytes: out.len() as u64,
            });
        }
    }
    out.push_str(&html[copied..]);
    if out.len() as u64 > VISUAL_PAGE_MAX_BYTES {
        return Err(PageError::PageTooLarge {
            bytes: out.len() as u64,
        });
    }
    Ok(out)
}

/// The file name of a page, from its title: `Tokens by week.html`.
///
/// `/ \ : * ? " < > |` and control characters become spaces, runs of spaces
/// become one, the ends are trimmed, and the name is cut to 120 characters.
/// A title with nothing left gives `visual.html`.
pub fn page_name(title: &str) -> String {
    const STEM_MAX_CHARS: usize = 120;
    let spaced: String = title
        .chars()
        .map(|c| {
            if c.is_control() || c.is_whitespace() || "/\\:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = collapsed.chars().take(STEM_MAX_CHARS).collect();
    let stem = cut.trim_end();
    if stem.is_empty() {
        "visual.html".to_owned()
    } else {
        format!("{stem}.html")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    fn refs(html: &str) -> Vec<(String, String)> {
        local_images(html)
            .into_iter()
            .map(|i| (i.reference, i.path))
            .collect()
    }

    fn one(reference: &str, path: &str) -> Vec<(String, String)> {
        vec![(reference.to_owned(), path.to_owned())]
    }

    fn image(reference: &str, path: &str) -> LocalImage {
        LocalImage {
            reference: reference.to_owned(),
            path: path.to_owned(),
        }
    }

    #[test]
    fn an_absolute_posix_path_is_local() {
        assert_eq!(
            refs(r#"<img src="/tmp/chart.png">"#),
            one("/tmp/chart.png", "/tmp/chart.png")
        );
        assert_eq!(
            refs("<IMG SRC='/tmp/a b.png' alt=x>"),
            one("/tmp/a b.png", "/tmp/a b.png")
        );
        assert_eq!(
            refs("<img src=/tmp/chart.png>"),
            one("/tmp/chart.png", "/tmp/chart.png")
        );
        assert_eq!(
            refs(r#"<img src="  /tmp/chart.png  ">"#),
            one("/tmp/chart.png", "/tmp/chart.png")
        );
    }

    #[test]
    fn a_file_url_is_decoded_to_a_path() {
        assert_eq!(
            refs(r#"<img src="file:///tmp/my%20chart.png">"#),
            one("file:///tmp/my%20chart.png", "/tmp/my chart.png")
        );
        assert_eq!(
            refs(r#"<img src="file://localhost/tmp/a.png">"#),
            one("file://localhost/tmp/a.png", "/tmp/a.png")
        );
        assert_eq!(
            refs(r#"<img src="FILE:///tmp/%C3%A9.png">"#),
            one("FILE:///tmp/%C3%A9.png", "/tmp/é.png")
        );
        assert_eq!(
            refs(r#"<img src="file:/tmp/a.png">"#),
            one("file:/tmp/a.png", "/tmp/a.png")
        );
        assert_eq!(
            refs(r#"<img src="file:///tmp/a.svg#icon">"#),
            one("file:///tmp/a.svg#icon", "/tmp/a.svg")
        );
        // A file URL for another host, a broken escape and a bare root.
        assert!(refs(r#"<img src="file://server/share/a.png">"#).is_empty());
        assert!(refs(r#"<img src="file:///tmp/%zz.png">"#).is_empty());
        assert!(refs(r#"<img src="file:///">"#).is_empty());
        assert!(refs(r#"<img src="file:relative.png">"#).is_empty());
    }

    #[test]
    fn a_windows_path_is_local() {
        assert_eq!(
            refs(r#"<img src="C:\Users\me\a.png">"#),
            one(r"C:\Users\me\a.png", r"C:\Users\me\a.png")
        );
        assert_eq!(
            refs(r#"<img src="d:/pics/a.png">"#),
            one("d:/pics/a.png", "d:/pics/a.png")
        );
        assert_eq!(
            refs(r#"<img src="file:///C:/pics/my%20a.png">"#),
            one("file:///C:/pics/my%20a.png", "C:/pics/my a.png")
        );
        // A scheme is not a drive letter.
        assert!(refs(r#"<img src="ab:/x.png">"#).is_empty());
        assert!(refs(r#"<img src="C:x.png">"#).is_empty());
    }

    #[test]
    fn other_references_are_not_local() {
        for src in [
            "http://example.com/a.png",
            "https://example.com/a.png",
            "HTTPS://example.com/a.png",
            "//example.com/a.png",
            "data:image/png;base64,AAAA",
            "blob:https://example.com/1234",
            "a.png",
            "./a.png",
            "../a.png",
            "img/a.png",
            "#icon",
            "/",
            "",
            "javascript:void(0)",
        ] {
            assert!(
                refs(&format!(r#"<img src="{src}">"#)).is_empty(),
                "{src:?} must not be local"
            );
        }
    }

    #[test]
    fn the_attributes_that_take_images_are_read() {
        let html = r#"
            <img src="/a/img.png">
            <picture><source src="/a/source.png"></picture>
            <svg><image href="/a/svg-href.png"/><image xlink:href="/a/svg-xlink.png"/></svg>
            <input type="image" src="/a/input.png">
            <video src="/a/video.mp4" poster="/a/poster.png"></video>
            <audio src="/a/audio.mp3"></audio>
        "#;
        let found: Vec<String> = refs(html).into_iter().map(|(r, _)| r).collect();
        assert_eq!(
            found,
            [
                "/a/img.png",
                "/a/source.png",
                "/a/svg-href.png",
                "/a/svg-xlink.png",
                "/a/input.png",
                "/a/video.mp4",
                "/a/poster.png",
                "/a/audio.mp3",
            ]
        );
    }

    #[test]
    fn the_attributes_that_take_no_image_are_skipped() {
        for html in [
            r#"<a href="/a/b.png">link</a>"#,
            r#"<img href="/a/b.png">"#,
            r#"<link rel="stylesheet" href="/a/b.css">"#,
            r#"<script src="/a/b.js"></script>"#,
            r#"<iframe src="/a/b.html"></iframe>"#,
            r#"<div poster="/a/b.png"></div>"#,
            r#"<div data-src="/a/b.png"></div>"#,
            r#"<div title="/a/b.png"></div>"#,
        ] {
            assert!(refs(html).is_empty(), "{html}");
        }
    }

    #[test]
    fn css_urls_in_style_blocks_and_attributes_are_read() {
        let html = r#"
            <style>
              .a { background: url(/a/plain.png) }
              .b { background: URL( "/a/double.png" ) }
              .c { background-image: url('/a/single.png'), url(https://example.com/x.png) }
              .d { background: url(file:///a/file%20url.png) }
            </style>
            <div style="background:url(/a/attr.png)"></div>
            <div style='background:url("/a/attr-quote.png")'></div>
            <div style="background:url(&quot;/a/attr-entity.png&quot;)"></div>
        "#;
        let found: Vec<(String, String)> = refs(html);
        let references: Vec<&str> = found.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(
            references,
            [
                "/a/plain.png",
                "/a/double.png",
                "/a/single.png",
                "file:///a/file%20url.png",
                "/a/attr.png",
                "/a/attr-quote.png",
                "/a/attr-entity.png",
            ]
        );
        assert_eq!(found[3].1, "/a/file url.png");
    }

    #[test]
    fn images_are_unique_and_in_the_order_of_the_page() {
        let html = r#"
            <div style="background:url(/b.png)"></div>
            <img src="/a.png">
            <img src="/b.png">
            <style>.x{background:url(/c.png)} .y{background:url(/a.png)}</style>
        "#;
        let found: Vec<String> = refs(html).into_iter().map(|(r, _)| r).collect();
        // The first use of each reference sets its place.
        assert_eq!(found, ["/b.png", "/a.png", "/c.png"]);
    }

    #[test]
    fn a_tag_in_a_style_block_or_in_text_is_not_a_use() {
        assert!(refs(r#"<style>.a::after{content:"<img src='/x.png'>"}</style>"#).is_empty());
        assert!(refs("<p>Write &lt;img src=\"/x.png\"&gt; in the page.</p>").is_empty());
    }

    #[test]
    fn script_code_does_not_hide_the_images_after_it() {
        let html = "<script>for(var i=0;i<n;i++){} // don't\nvar t = '<img src=\"/in-script.png\">';</script><img src=\"/after.png\">";
        assert_eq!(refs(html), one("/after.png", "/after.png"));
    }

    #[test]
    fn each_image_type_is_sniffed_by_its_magic_bytes() {
        assert_eq!(sniff_image(PNG), Some("image/png"));
        assert_eq!(
            sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_image(b"GIF89a\x01\x00"), Some("image/gif"));
        assert_eq!(sniff_image(b"GIF87a\x01\x00"), Some("image/gif"));
        assert_eq!(
            sniff_image(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(
            sniff_image(br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#),
            Some("image/svg+xml")
        );
    }

    #[test]
    fn svg_text_may_start_with_a_bom_a_declaration_comments_and_a_doctype() {
        let bom = "\u{feff}  <svg viewBox='0 0 1 1'/>";
        assert_eq!(sniff_image(bom.as_bytes()), Some("image/svg+xml"));
        let full = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- made by hand -->\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n<!-- more -->\n<svg\n xmlns=\"http://www.w3.org/2000/svg\"/>";
        assert_eq!(sniff_image(full.as_bytes()), Some("image/svg+xml"));
        let subset = "<!DOCTYPE svg [ <!ENTITY a \"b\"> ]><svg/>";
        assert_eq!(sniff_image(subset.as_bytes()), Some("image/svg+xml"));
        assert_eq!(sniff_image(b"<svg>"), Some("image/svg+xml"));
    }

    #[test]
    fn a_renamed_file_is_not_an_image() {
        for bytes in [
            &b""[..],
            b"hello",
            b"#!/bin/sh\nrm -rf /\n",
            b"-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n",
            b"{\"token\":\"secret\"}",
            b"<html><body><svg></svg></body></html>",
            b"<svgx/>",
            b"<?xml version=\"1.0\"?><note/>",
            b"<!-- never closed <svg/>",
            b"<?xml never closed <svg/>",
            b"%PDF-1.7",
            b"PK\x03\x04",
            // A PNG header cut short.
            b"\x89PNG\r\n",
            // Text with a NUL byte.
            b"<svg/>\0",
            // Not UTF-8.
            b"<svg/>\xFF\xFE",
            // RIFF, but not WebP.
            b"RIFF\x24\x00\x00\x00WAVEfmt ",
        ] {
            assert_eq!(
                sniff_image(bytes),
                None,
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn inlining_replaces_every_use_and_keeps_references_with_no_file() {
        let html = r#"<img src="/a.png"><img src="/a.png"><img src="/other.png">
            <style>.x{background:url(/a.png)}</style>
            <div style="background:url('/a.png')"></div>"#;
        let files = vec![(image("/a.png", "/a.png"), PNG.to_vec())];
        let out = inline_images(html, &files).expect("inlines");
        let uri = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
        assert_eq!(out.matches(&uri).count(), 4);
        assert!(!out.contains("\"/a.png\""));
        assert!(out.contains(r#"<img src="/other.png">"#));
        assert!(out.contains(&format!("url({uri})")));
        assert!(out.contains(&format!("url('{uri}')")));
        assert!(out.contains(&format!(r#"<img src="{uri}"><img src="{uri}">"#)));
    }

    #[test]
    fn inlining_uses_the_type_found_in_the_bytes() {
        let html = r#"<img src="/photo.png">"#;
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#.to_vec();
        let files = vec![(image("/photo.png", "/photo.png"), svg.clone())];
        let out = inline_images(html, &files).expect("inlines");
        assert_eq!(
            out,
            format!(
                r#"<img src="data:image/svg+xml;base64,{}">"#,
                STANDARD.encode(&svg)
            )
        );
    }

    #[test]
    fn inlining_does_not_touch_a_longer_path_with_the_same_start() {
        let html = r#"<img src="/a.png"><img src="/a.png.bak"><img src="/dir/a.png">"#;
        let files = vec![(image("/a.png", "/a.png"), PNG.to_vec())];
        let out = inline_images(html, &files).expect("inlines");
        assert!(out.contains(r#"<img src="/a.png.bak">"#));
        assert!(out.contains(r#"<img src="/dir/a.png">"#));
        assert_eq!(out.matches("data:image/png").count(), 1);
    }

    #[test]
    fn inlining_a_file_url_replaces_the_url_as_written() {
        let html = r#"<img src="file:///tmp/my%20a.png">"#;
        let files = vec![(
            image("file:///tmp/my%20a.png", "/tmp/my a.png"),
            PNG.to_vec(),
        )];
        let out = inline_images(html, &files).expect("inlines");
        assert!(out.starts_with(r#"<img src="data:image/png;base64,"#));
        assert!(!out.contains("file:"));
    }

    #[test]
    fn a_page_with_no_files_is_returned_as_it_is() {
        let html = r#"<img src="/a.png"> <p>text</p>"#;
        assert_eq!(inline_images(html, &[]).as_deref(), Ok(html));
    }

    #[test]
    fn an_image_over_the_limit_is_refused_even_when_it_is_a_png() {
        let limit = usize::try_from(VISUAL_IMAGE_MAX_BYTES).expect("fits");
        let mut at_limit = PNG.to_vec();
        at_limit.resize(limit, 0);
        let files = vec![(image("/a.png", "/a.png"), at_limit.clone())];
        // One image at the limit and unused in the page: it passes.
        assert!(inline_images("<p>x</p>", &files).is_ok());
        let mut over = at_limit;
        over.push(0);
        let files = vec![(image("/a.png", "/a.png"), over)];
        assert_eq!(
            inline_images("<img src=\"/a.png\">", &files),
            Err(PageError::ImageTooLarge {
                path: "/a.png".to_owned()
            })
        );
        // The size is checked before the type.
        let files = vec![(image("/big.txt", "/big.txt"), vec![b'a'; limit + 1])];
        assert_eq!(
            inline_images("<img src=\"/big.txt\">", &files),
            Err(PageError::ImageTooLarge {
                path: "/big.txt".to_owned()
            })
        );
    }

    #[test]
    fn a_file_that_is_not_an_image_is_refused_with_its_path() {
        let files = vec![(
            image("/home/me/.ssh/id_rsa", "/home/me/.ssh/id_rsa"),
            b"-----BEGIN OPENSSH PRIVATE KEY-----".to_vec(),
        )];
        let err = inline_images("<img src=\"/home/me/.ssh/id_rsa\">", &files).expect_err("refused");
        assert_eq!(
            err,
            PageError::NotAnImage {
                path: "/home/me/.ssh/id_rsa".to_owned()
            }
        );
        let message = err.to_string();
        assert!(message.contains("/home/me/.ssh/id_rsa"));
        assert!(message.contains("PNG, JPEG, GIF, WebP or SVG"));
    }

    #[test]
    fn a_page_over_the_limit_is_refused() {
        let limit = usize::try_from(VISUAL_IMAGE_MAX_BYTES).expect("fits");
        let mut big = PNG.to_vec();
        big.resize(limit, 0);
        // One 10 MiB image is about 14 MB as base64. Two uses are over 25 MiB.
        let html = r#"<img src="/a.png"><img src="/a.png">"#;
        let files = vec![(image("/a.png", "/a.png"), big.clone())];
        match inline_images(html, &files) {
            Err(PageError::PageTooLarge { bytes }) => assert!(bytes > VISUAL_PAGE_MAX_BYTES),
            other => panic!("wrong result: {other:?}"),
        }
        // One use fits.
        let html = r#"<img src="/a.png">"#;
        let out = inline_images(html, &files).expect("fits");
        assert!(out.len() as u64 <= VISUAL_PAGE_MAX_BYTES);
        let message = PageError::PageTooLarge { bytes: 1 }.to_string();
        assert!(message.contains("25 MiB"));
    }

    #[test]
    fn page_names_are_safe_file_names() {
        assert_eq!(page_name("Tokens by week"), "Tokens by week.html");
        assert_eq!(page_name("  A   B  "), "A B.html");
        assert_eq!(
            page_name("a/b\\c:d*e?f\"g<h>i|j"),
            "a b c d e f g h i j.html"
        );
        assert_eq!(page_name("a\tb\nc\u{7}d"), "a b c d.html");
        assert_eq!(page_name("Q3: costs / revenue"), "Q3 costs revenue.html");
        assert_eq!(page_name("Übersicht 📈"), "Übersicht 📈.html");
        assert_eq!(page_name(""), "visual.html");
        assert_eq!(page_name("   "), "visual.html");
        assert_eq!(page_name("/\\:*?\"<>|"), "visual.html");
        assert_eq!(page_name("..."), "....html");
    }

    #[test]
    fn page_names_are_cut_to_120_characters() {
        let title = "é".repeat(300);
        let name = page_name(&title);
        assert_eq!(name, format!("{}.html", "é".repeat(120)));
        // A cut that ends in a space leaves no space before the extension.
        let title = format!("{} {}", "a".repeat(119), "b".repeat(50));
        assert_eq!(page_name(&title), format!("{}.html", "a".repeat(119)));
    }
}
