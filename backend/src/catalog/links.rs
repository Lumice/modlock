//! Bounded native projection of anchors in pinned Profile/Post/Update text.
//! This scanner emits validated link metadata, never HTML or executable markup.
use super::provider;
use json::{object, JsonValue};

pub fn project(markup: &str) -> (JsonValue, bool) {
    let bytes = markup.as_bytes();
    let mut cursor = 0;
    let mut pending: Option<(String, usize)> = None;
    let mut links = Vec::new();
    let mut truncated = false;
    while cursor < bytes.len() {
        if bytes[cursor] != b'<' {
            cursor += 1;
            continue;
        }
        let start = cursor;
        if bytes[cursor..].starts_with(b"<!--") {
            cursor = bytes[cursor + 4..]
                .windows(3)
                .position(|w| w == b"-->")
                .map(|n| cursor + 4 + n + 3)
                .unwrap_or(bytes.len());
            continue;
        }
        let mut end = cursor + 1;
        let mut quote = 0;
        while end < bytes.len() {
            let b = bytes[end];
            if quote != 0 {
                if b == quote {
                    quote = 0;
                }
            } else if b == b'\'' || b == b'"' {
                quote = b;
            } else if b == b'>' {
                break;
            }
            end += 1;
        }
        if end == bytes.len() {
            truncated = true;
            break;
        }
        let mut at = start + 1;
        while at < end && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let closing = at < end && bytes[at] == b'/';
        if closing {
            at += 1;
        }
        let name_start = at;
        while at < end && !bytes[at].is_ascii_whitespace() && bytes[at] != b'/' {
            at += 1;
        }
        let name = &bytes[name_start..at];
        if !closing && (name.eq_ignore_ascii_case(b"script") || name.eq_ignore_ascii_case(b"style"))
        {
            let needle = if name.eq_ignore_ascii_case(b"script") {
                b"</script".as_slice()
            } else {
                b"</style".as_slice()
            };
            cursor = bytes[end + 1..]
                .windows(needle.len())
                .position(|w| w.eq_ignore_ascii_case(needle))
                .map(|n| end + 1 + n)
                .unwrap_or(bytes.len());
            continue;
        }
        if name.eq_ignore_ascii_case(b"a") {
            if let Some((url, label_start)) = pending.take() {
                if !closing {
                    truncated = true;
                }
                push(&mut links, url, &markup[label_start..start], &mut truncated);
            }
            if !closing {
                let mut href: Option<&str> = None;
                let mut duplicate = false;
                while at < end {
                    while at < end && (bytes[at].is_ascii_whitespace() || bytes[at] == b'/') {
                        at += 1;
                    }
                    let attr_start = at;
                    while at < end
                        && !bytes[at].is_ascii_whitespace()
                        && !b"=/>".contains(&bytes[at])
                    {
                        at += 1;
                    }
                    if attr_start == at {
                        at += 1;
                        continue;
                    }
                    let is_href = bytes[attr_start..at].eq_ignore_ascii_case(b"href");
                    while at < end && bytes[at].is_ascii_whitespace() {
                        at += 1;
                    }
                    if at == end || bytes[at] != b'=' {
                        continue;
                    }
                    at += 1;
                    while at < end && bytes[at].is_ascii_whitespace() {
                        at += 1;
                    }
                    let quoted = if at < end && matches!(bytes[at], b'\'' | b'"') {
                        let q = bytes[at];
                        at += 1;
                        Some(q)
                    } else {
                        None
                    };
                    let value_start = at;
                    while at < end
                        && !quoted
                            .map_or_else(|| bytes[at].is_ascii_whitespace(), |q| bytes[at] == q)
                    {
                        at += 1;
                    }
                    if is_href {
                        duplicate |= href.is_some();
                        href = Some(&markup[value_start..at]);
                    }
                    if quoted.is_some() && at < end {
                        at += 1;
                    }
                }
                if duplicate {
                    truncated = true;
                } else if let Some(raw) = href {
                    let decoded = raw
                        .replace("&amp;", "&")
                        .replace("&quot;", "\"")
                        .replace("&apos;", "'")
                        .replace("&#39;", "'")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">");
                    let url = if decoded.starts_with('/') && !decoded.starts_with("//") {
                        format!("https://gamebanana.com{decoded}")
                    } else if decoded
                        .get(..8)
                        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
                    {
                        format!("https://{}", &decoded[8..])
                    } else {
                        decoded
                    };
                    let entity_left = url.match_indices('&').any(|(at, _)| {
                        let tail = &url.as_bytes()[at + 1..];
                        let len = tail
                            .iter()
                            .take(32)
                            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'#')
                            .count();
                        len > 0 && tail.get(len) == Some(&b';')
                    });
                    if url.len() <= 2048
                        && provider::https(&url)
                        && !url.contains(['<', '>', '"', '\''])
                        && !entity_left
                    {
                        pending = Some((url, end + 1));
                    } else {
                        truncated = true;
                    }
                }
            }
        }
        cursor = end + 1;
    }
    if let Some((url, start)) = pending {
        truncated = true;
        push(&mut links, url, &markup[start..], &mut truncated);
    }
    (links.into(), truncated)
}
pub fn validate(value: &JsonValue) -> Result<(), crate::native::NativeError> {
    use super::{contract, store::storage};
    if !value["links"].is_array()
        || value["links"].len() > 32
        || !value["linksTruncated"].is_boolean()
    {
        return Err(storage());
    }
    for link in value["links"].members() {
        contract::exact(link, &["label", "url"]).map_err(|_| storage())?;
        if link["label"].as_str().is_none_or(|s| s.len() > 256)
            || link["url"]
                .as_str()
                .is_none_or(|s| s.len() > 2048 || !provider::https(s))
        {
            return Err(storage());
        }
    }
    Ok(())
}
fn push(links: &mut Vec<JsonValue>, url: String, markup: &str, truncated: &mut bool) {
    if links.iter().any(|link| link["url"] == url.as_str()) {
        return;
    }
    if links.len() == 32 {
        *truncated = true;
        return;
    }
    let text = provider::plain(markup);
    let mut label = if text.trim().is_empty() {
        url.clone()
    } else {
        text.trim().to_owned()
    };
    if label.len() > 256 {
        let mut end = 256;
        while !label.is_char_boundary(end) {
            end -= 1;
        }
        label.truncate(end);
        *truncated = true;
    }
    links.push(object! {label:label,url:url});
}
