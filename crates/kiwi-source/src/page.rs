//! Download pages: the AltStore sources, IPAs and icon a web page links to.
//!
//! A small tag scanner rather than an HTML parser: Kiwi only needs attribute values, and a
//! page that hides its links behind scripts has to be added by its source or IPA URL.

use crate::Url;

/// Links found on a page, absolute and in document order, without duplicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageLinks {
    /// AltStore-format sources, from `altstore://source?url=…`-style buttons.
    pub sources: Vec<Url>,
    /// HTTP(S) links whose path ends in `.ipa`.
    pub ipas: Vec<Url>,
    /// The page's Home Screen icon (`apple-touch-icon`), the largest one declared.
    pub icon: Option<Url>,
}

/// Apps that add sources from a link: `<scheme>://source?url=<source URL>`.
const SOURCE_SCHEMES: &[&str] = &["altstore", "altstore-classic", "sidestore"];

/// Largest page scanned; download pages are small.
pub const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;

pub fn page_links(html: &str, base: &Url) -> PageLinks {
    let mut links = PageLinks::default();
    let mut icon_size = 0u32;
    let html = &html[..floor_char_boundary(html, MAX_PAGE_BYTES)];
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if let Some(comment) = rest.strip_prefix("!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        rest = &rest[end..];
        let name_len = tag
            .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
            .unwrap_or(tag.len());
        let name = tag[..name_len].to_ascii_lowercase();
        // Script and style bodies are not markup.
        if name == "script" || name == "style" {
            let close = format!("</{name}");
            rest = rest
                .to_ascii_lowercase()
                .find(&close)
                .map_or("", |i| &rest[i..]);
            continue;
        }
        let attrs = attributes(&tag[name_len..]);
        let attr = |key: &str| {
            attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        let Some(href) = attr("href") else { continue };
        let Ok(url) = base.join(href.trim()) else {
            continue;
        };
        if name == "link" {
            let rel = attr("rel").unwrap_or_default().to_ascii_lowercase();
            if rel
                .split_ascii_whitespace()
                .any(|r| matches!(r, "apple-touch-icon" | "apple-touch-icon-precomposed"))
                && let Some(url) = web_url(url)
            {
                let size = attr("sizes")
                    .and_then(|s| s.split(['x', 'X']).next()?.trim().parse().ok())
                    .unwrap_or(180);
                if links.icon.is_none() || size > icon_size {
                    links.icon = Some(url);
                    icon_size = size;
                }
            }
            continue;
        }
        if SOURCE_SCHEMES.contains(&url.scheme()) {
            if let Some(source) = url
                .query_pairs()
                .find(|(k, _)| k == "url")
                .and_then(|(_, v)| web_url(Url::parse(v.trim()).ok()?))
                && !links.sources.contains(&source)
            {
                links.sources.push(source);
            }
            continue;
        }
        if url.path().to_ascii_lowercase().ends_with(".ipa")
            && let Some(mut ipa) = web_url(url)
        {
            ipa.set_fragment(None);
            if !links.ipas.contains(&ipa) {
                links.ipas.push(ipa);
            }
        }
    }
    links
}

/// HTTP(S) URLs only, upgraded to HTTPS (see [`crate::source_input`]).
fn web_url(mut url: Url) -> Option<Url> {
    match url.scheme() {
        "https" => Some(url),
        "http" => {
            if !crate::urls::is_loopback(&url) {
                url.set_scheme("https").ok()?;
            }
            Some(url)
        }
        _ => None,
    }
}

/// End of a tag's text: the first `>` outside a quoted attribute value.
fn tag_end(tag: &str) -> usize {
    let mut quote = None;
    for (i, c) in tag.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return i,
            _ => {}
        }
    }
    tag.len()
}

/// `key=value` pairs (lowercase keys, entity-decoded values) of a tag's text.
fn attributes(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_ascii_whitespace() || c == '/' {
            chars.next();
            continue;
        }
        let mut end = i;
        while let Some(&(j, c)) = chars.peek() {
            if c.is_ascii_whitespace() || c == '=' || c == '/' {
                break;
            }
            end = j + c.len_utf8();
            chars.next();
        }
        let key = text[i..end].to_ascii_lowercase();
        while chars.peek().is_some_and(|(_, c)| c.is_ascii_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none_or(|(_, c)| *c != '=') {
            if !key.is_empty() {
                out.push((key, String::new()));
            }
            if end == i {
                chars.next();
            }
            continue;
        }
        chars.next();
        while chars.peek().is_some_and(|(_, c)| c.is_ascii_whitespace()) {
            chars.next();
        }
        let value = match chars.peek().copied() {
            Some((j, q @ ('"' | '\''))) => {
                chars.next();
                let mut close = text.len();
                for (k, c) in chars.by_ref() {
                    if c == q {
                        close = k;
                        break;
                    }
                }
                &text[j + 1..close]
            }
            Some((j, _)) => {
                let mut close = text.len();
                while let Some(&(k, c)) = chars.peek() {
                    if c.is_ascii_whitespace() {
                        close = k;
                        break;
                    }
                    chars.next();
                }
                &text[j..close]
            }
            None => "",
        };
        out.push((key, decode_entities(value)));
    }
    out
}

/// The character references that appear in URLs.
fn decode_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "lt" => Some('<'),
            "gt" => Some('>'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#')?.parse().ok())
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    (0..=max)
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/download/").unwrap()
    }

    #[test]
    fn finds_sources_ipas_and_the_largest_touch_icon() {
        let html = r#"<!doctype html><html><head>
<link rel="apple-touch-icon" href="/touch-120.png" sizes="120x120">
<link rel=apple-touch-icon sizes="180x180" href='/touch-180.png'>
<link rel="icon" href="/favicon.png">
<script>var a = '<a href="/script.ipa">';</script>
</head><body>
<!-- <a href="/commented.ipa"> -->
<a class="btn" href="sidestore://source?url=https%3A%2F%2Fexample.com%2Fsource.json">SideStore</a>
<a href="altstore://source?url=https%3A%2F%2Fexample.com%2Fsource.json">AltStore</a>
<a href="altstore://source?url=http%3A%2F%2Fother.example%2Fapps.json&amp;x=1">Other</a>
<a href="https://example.com/downloads/App-v1.2.ipa" download>IPA</a>
<a href="../downloads/App-v1.2.ipa#top">again</a>
<a href="http://cdn.example.com/Old.IPA?token=a&amp;b=c">old</a>
<a href="itms-services://?action=download-manifest&url=https://example.com/m.plist">OTA</a>
<a href="javascript:void(0)">nothing</a>
<a href="/downloads/App-mac.zip">Mac</a>
</body></html>"#;
        let links = page_links(html, &base());
        assert_eq!(
            links.sources,
            [
                Url::parse("https://example.com/source.json").unwrap(),
                Url::parse("https://other.example/apps.json").unwrap()
            ]
        );
        assert_eq!(
            links.ipas,
            [
                Url::parse("https://example.com/downloads/App-v1.2.ipa").unwrap(),
                Url::parse("https://cdn.example.com/Old.IPA?token=a&b=c").unwrap()
            ]
        );
        assert_eq!(
            links.icon,
            Some(Url::parse("https://example.com/touch-180.png").unwrap())
        );
    }

    #[test]
    fn tolerates_broken_markup() {
        for html in [
            "",
            "<",
            "<a href=",
            "<a href=\"unterminated.ipa",
            "<!-- never closed <a href=x.ipa>",
            "<a href=&#x2F;x.ipa>",
            "<a href=\"&amp\">",
            "<a href='&#99999999;'>",
            "<script>",
            "<a hr\u{e9}f=\"\u{e9}.ipa\">",
        ] {
            page_links(html, &base());
        }
        assert_eq!(
            page_links("<a href=&#x2F;x.ipa>", &base()).ipas,
            [Url::parse("https://example.com/x.ipa").unwrap()]
        );
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("a&amp;b&#38;c&#x26;d&nope;e&"),
            "a&b&c&d&nope;e&"
        );
    }
}
