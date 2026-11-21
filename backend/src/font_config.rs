//! Conservative UTF-8 XML admission and byte-preserving native font blocks.
//! This does not resolve a DTD/include or establish game font matching semantics.
use crate::{fonts::FontBlock, native::NativeError, package};
use quick_xml::{events::Event, Reader};

pub const MAX_CONF_BYTES: usize = 4 * 1024 * 1024;
const MARKER: &str = "deadlock-native-fonts:";
pub const MISSING_CONF_WARNING: &str =
    "Font files will be copied; fonts.conf is missing, so no font patterns will be added.";
fn invalid() -> NativeError {
    NativeError::new("invalidFontConfiguration", "fonts.conf is unsupported, malformed, or its native block differs from recorded bytes; preserve it and review the configuration.")
}
pub(crate) fn xml_chars(s: &str) -> bool {
    s.chars().all(
        |c| matches!(c as u32, 9 | 10 | 13 | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff),
    )
}
// XML 1.0 Fifth Edition Name, with namespace names left uninterpreted.
fn name_start(c: char) -> bool {
    matches!(c as u32, 0x3a | 0x41..=0x5a | 0x5f | 0x61..=0x7a | 0xc0..=0xd6 | 0xd8..=0xf6 | 0xf8..=0x2ff | 0x370..=0x37d | 0x37f..=0x1fff | 0x200c..=0x200d | 0x2070..=0x218f | 0x2c00..=0x2fef | 0x3001..=0xd7ff | 0xf900..=0xfdcf | 0xfdf0..=0xfffd | 0x10000..=0xeffff)
}
fn name(s: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(s) else {
        return false;
    };
    let mut chars = s.chars();
    chars.next().is_some_and(name_start) && chars.all(|c| name_start(c) || matches!(c as u32, 0x2d | 0x2e | 0x30..=0x39 | 0xb7 | 0x300..=0x36f | 0x203f..=0x2040))
}
fn whitespace(s: &str) -> bool {
    s.bytes().all(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
}
fn attributes_spaced(raw: &[u8], mut pos: usize) -> bool {
    // quick-xml deliberately accepts adjacent quoted attributes; XML 1.0
    // requires whitespace before every attribute, including declaration fields.
    let space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    while pos < raw.len() {
        if !space(raw[pos]) {
            return false;
        }
        while pos < raw.len() && space(raw[pos]) {
            pos += 1;
        }
        if pos == raw.len() {
            return true;
        }
        let key = pos;
        while pos < raw.len() && !space(raw[pos]) && raw[pos] != b'=' {
            pos += 1;
        }
        if !name(&raw[key..pos]) {
            return false;
        }
        while pos < raw.len() && space(raw[pos]) {
            pos += 1;
        }
        if raw.get(pos) != Some(&b'=') {
            return false;
        }
        pos += 1;
        while pos < raw.len() && space(raw[pos]) {
            pos += 1;
        }
        let Some(&quote @ (b'\'' | b'"')) = raw.get(pos) else {
            return false;
        };
        pos += 1;
        while pos < raw.len() && raw[pos] != quote {
            pos += 1;
        }
        if pos == raw.len() {
            return false;
        }
        pos += 1;
    }
    true
}
fn decoded(s: &str) -> Result<(), NativeError> {
    let text = quick_xml::escape::unescape(s).map_err(|_| invalid())?;
    if !xml_chars(&text) {
        return Err(invalid());
    }
    Ok(())
}
fn doctype(s: &str) -> bool {
    // Only the external declaration used by the game contract is admitted.
    // No internal subset, PUBLIC identifier, alternate path or entity expansion.
    let s = s.trim_matches([' ', '\t', '\r', '\n']);
    let Some(rest) = s.strip_prefix("fontconfig") else {
        return false;
    };
    if !rest.starts_with([' ', '\t', '\r', '\n']) {
        return false;
    }
    let Some(rest) = rest
        .trim_start_matches([' ', '\t', '\r', '\n'])
        .strip_prefix("SYSTEM")
    else {
        return false;
    };
    if !rest.starts_with([' ', '\t', '\r', '\n']) {
        return false;
    }
    matches!(
        rest.trim_matches([' ', '\t', '\r', '\n']),
        "\"fonts.dtd\"" | "'fonts.dtd'"
    )
}
struct Parsed {
    insertion: usize,
    block: Option<(usize, usize)>,
}
fn parse(bytes: &[u8], expected: Option<&FontBlock>) -> Result<Parsed, NativeError> {
    if bytes.len() > MAX_CONF_BYTES {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    if !xml_chars(text) {
        return Err(invalid());
    }
    let bom = usize::from(bytes.starts_with(&[0xef, 0xbb, 0xbf])) * 3;
    let mut reader = Reader::from_reader(&bytes[bom..]);
    reader.config_mut().check_comments = true;
    reader.config_mut().check_end_names = true;
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let (mut root_seen, mut closed, mut declaration, mut dtd) = (false, false, false, false);
    let (mut last_pattern, mut close, mut begin, mut block) = (None, None, None, None);
    loop {
        let start = bom + reader.buffer_position() as usize;
        let event = reader.read_event().map_err(|_| invalid())?;
        let end = bom + reader.buffer_position() as usize;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                if closed
                    || !name(e.name().as_ref())
                    || !attributes_spaced(e.as_ref(), e.name().as_ref().len())
                    || stack.len() >= 64
                {
                    return Err(invalid());
                }
                for attr in e.attributes().with_checks(true) {
                    let attr = attr.map_err(|_| invalid())?;
                    if !name(attr.key.as_ref()) || attr.value.contains(&b'<') {
                        return Err(invalid());
                    }
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| invalid())?;
                    if !xml_chars(&value) {
                        return Err(invalid());
                    }
                }
                if stack.is_empty() {
                    // A self-closing root has no byte offset at which to insert a child
                    // without changing original syntax, so it is conservatively rejected.
                    if root_seen || e.name().as_ref() != b"fontconfig" || empty {
                        return Err(invalid());
                    }
                    root_seen = true;
                }
                if empty {
                    if stack.len() == 1 && e.name().as_ref() == b"fontpattern" && begin.is_none() {
                        last_pattern = Some(end);
                    }
                } else {
                    stack.push(e.name().as_ref().to_vec());
                }
            }
            Event::End(e) => {
                if stack.last().map(Vec::as_slice) != Some(e.name().as_ref()) {
                    return Err(invalid());
                }
                if stack.len() == 2 && e.name().as_ref() == b"fontpattern" && begin.is_none() {
                    last_pattern = Some(end);
                }
                if stack.len() == 1 {
                    close = Some(start);
                    closed = true;
                }
                stack.pop();
            }
            Event::Text(e) => {
                let raw = std::str::from_utf8(e.as_ref()).map_err(|_| invalid())?;
                if raw.contains("]]>") || (stack.is_empty() && !whitespace(raw)) {
                    return Err(invalid());
                }
                decoded(raw)?;
            }
            Event::GeneralRef(e) => {
                if stack.is_empty() {
                    return Err(invalid());
                }
                let raw = std::str::from_utf8(e.as_ref()).map_err(|_| invalid())?;
                decoded(&format!("&{raw};"))?;
            }
            Event::CData(_) => {
                if stack.is_empty() || !bytes[start..end].starts_with(b"<![CDATA[") {
                    return Err(invalid());
                }
            }
            Event::Comment(e) => {
                let comment = std::str::from_utf8(e.as_ref()).map_err(|_| invalid())?;
                if comment.contains("deadlock-native-fonts") {
                    let expected = expected.ok_or_else(invalid)?;
                    if stack.len() != 1 {
                        return Err(invalid());
                    }
                    if comment == marker(expected, "begin") {
                        if begin.is_some() || block.is_some() {
                            return Err(invalid());
                        }
                        // The recorded leading line ending is part of the owned span.
                        let prefix = if expected.bytes.starts_with(b"\r\n") {
                            2
                        } else {
                            1
                        };
                        let block_start = start.checked_sub(prefix).ok_or_else(invalid)?;
                        begin = Some(block_start);
                    } else if comment == marker(expected, "end") {
                        let first = begin.take().ok_or_else(invalid)?;
                        if block.is_some() || &bytes[first..end] != expected.bytes.as_slice() {
                            return Err(invalid());
                        }
                        block = Some((first, end));
                    } else {
                        return Err(invalid());
                    }
                }
            }
            Event::Decl(e) => {
                if declaration
                    || root_seen
                    || dtd
                    || start != bom
                    || e.version().map_err(|_| invalid())?.as_ref() != b"1.0"
                {
                    return Err(invalid());
                }
                if let Some(enc) = e.encoding() {
                    if !enc.map_err(|_| invalid())?.eq_ignore_ascii_case(b"utf-8") {
                        return Err(invalid());
                    }
                }
                // Enforce full declaration grammar; quick-xml's accessors alone
                // do not reject duplicate or unknown pseudo-attributes.
                let raw = std::str::from_utf8(e.as_ref()).map_err(|_| invalid())?;
                if !attributes_spaced(raw.as_bytes(), 3) {
                    return Err(invalid());
                }
                let start_tag = quick_xml::events::BytesStart::from_content(raw, 3);
                let mut keys = Vec::new();
                for a in start_tag.attributes().with_checks(true) {
                    let a = a.map_err(|_| invalid())?;
                    let key = a.key.as_ref().to_vec();
                    let value = a.value.as_ref();
                    match key.as_slice() {
                        b"version" if keys.is_empty() && value == b"1.0" => (),
                        b"encoding" if keys.len() == 1 && value.eq_ignore_ascii_case(b"utf-8") => {
                            ()
                        }
                        b"standalone"
                            if (keys.len() == 1 || keys.len() == 2)
                                && matches!(value, b"yes" | b"no") =>
                        {
                            ()
                        }
                        _ => return Err(invalid()),
                    }
                    keys.push(key);
                }
                if keys.is_empty() {
                    return Err(invalid());
                }
                declaration = true;
            }
            Event::DocType(e) => {
                if dtd
                    || root_seen
                    || !bytes[start..end].starts_with(b"<!DOCTYPE")
                    || !bytes
                        .get(start + 9)
                        .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
                    || !doctype(std::str::from_utf8(e.as_ref()).map_err(|_| invalid())?)
                {
                    return Err(invalid());
                }
                dtd = true;
            }
            Event::PI(e) => {
                if !name(e.target()) || e.target().eq_ignore_ascii_case(b"xml") {
                    return Err(invalid());
                }
            }
            Event::Eof => break,
        }
    }
    if !root_seen
        || !closed
        || !stack.is_empty()
        || begin.is_some()
        || expected.is_some() != block.is_some()
        || bom + reader.buffer_position() as usize != bytes.len()
    {
        return Err(invalid());
    }
    Ok(Parsed {
        insertion: last_pattern.or(close).ok_or_else(invalid)?,
        block,
    })
}
fn marker(block: &FontBlock, edge: &str) -> String {
    format!("{MARKER}{}:{}:{edge}", block.root_id, block.id)
}
pub(crate) fn render_block(
    root_id: &str,
    id: &str,
    names: Vec<String>,
    crlf: bool,
) -> Result<FontBlock, NativeError> {
    if !package::hex(root_id, 64)
        || !package::hex(id, 32)
        || names.is_empty()
        || names.windows(2).any(|w| w[0] >= w[1])
        || names.iter().any(|n| {
            n.is_empty()
                || n.len() > 1024
                || n.chars().count() > 256
                || !xml_chars(n)
                || n.chars().any(char::is_control)
        })
    {
        return Err(invalid());
    }
    let mut block = FontBlock {
        root_id: root_id.into(),
        id: id.into(),
        names,
        bytes: Vec::new(),
    };
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut bytes = format!("{nl}<!--{}-->", marker(&block, "begin")).into_bytes();
    for name in &block.names {
        bytes.extend_from_slice(
            format!(
                "{nl}  <fontpattern>{}</fontpattern>",
                quick_xml::escape::escape(name)
            )
            .as_bytes(),
        );
        if bytes.len() > MAX_CONF_BYTES {
            return Err(invalid());
        }
    }
    bytes.extend_from_slice(format!("{nl}<!--{}-->", marker(&block, "end")).as_bytes());
    if bytes.len() > MAX_CONF_BYTES {
        return Err(invalid());
    }
    block.bytes = bytes;
    Ok(block)
}
pub(crate) fn validate_block(block: &FontBlock) -> Result<(), NativeError> {
    if render_block(
        &block.root_id,
        &block.id,
        block.names.clone(),
        block.bytes.starts_with(b"\r\n"),
    )?
    .bytes
        != block.bytes
    {
        return Err(invalid());
    }
    Ok(())
}
pub fn patch_fonts_conf(
    before: Option<&[u8]>,
    expected_block: Option<&FontBlock>,
    next_block: Option<&FontBlock>,
) -> Result<Option<Vec<u8>>, NativeError> {
    if let Some(block) = expected_block {
        validate_block(block)?;
    }
    if let Some(block) = next_block {
        validate_block(block)?;
    }
    let Some(before) = before else {
        if expected_block.is_some() {
            return Err(invalid());
        }
        return Ok(None);
    };
    let parsed = parse(before, expected_block)?;
    if expected_block == next_block {
        return Ok(Some(before.to_vec()));
    }
    let mut outside = Vec::with_capacity(before.len());
    if let Some((start, end)) = parsed.block {
        outside.extend_from_slice(&before[..start]);
        outside.extend_from_slice(&before[end..]);
    } else {
        outside.extend_from_slice(before);
    }
    // Reparse the retained slices so the insertion offset follows the final
    // unrelated direct-root fontpattern even when the old block moved.
    let insertion = parse(&outside, None)?.insertion;
    if let Some(block) = next_block {
        if outside
            .len()
            .checked_add(block.bytes.len())
            .is_none_or(|n| n > MAX_CONF_BYTES)
        {
            return Err(invalid());
        }
        let mut after = Vec::with_capacity(outside.len() + block.bytes.len());
        after.extend_from_slice(&outside[..insertion]);
        after.extend_from_slice(&block.bytes);
        after.extend_from_slice(&outside[insertion..]);
        parse(&after, Some(block))?;
        Ok(Some(after))
    } else {
        Ok(Some(outside))
    }
}
