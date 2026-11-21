//! Conservative KeyValues parser retaining byte spans: only direct, reviewed
//! SearchPaths mount lines are removed. The exact original-manager addons
//! line is a reviewed takeover; its folder is never modified. Everything else
//! is byte-for-byte intact.
use crate::native::NativeError;
use std::collections::HashSet;
#[derive(Debug)]
pub(crate) struct Token {
    pub(crate) text: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}
fn bad() -> NativeError {
    NativeError::new("invalidGameinfo", "Gameinfo must contain one unambiguous FileSystem/SearchPaths block and ordinary mount lines.")
}
pub(crate) fn tokens(s: &str) -> Result<Vec<Token>, NativeError> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1
            }
            continue;
        }
        let start = i;
        let text;
        if b[i] == b'"' {
            i += 1;
            let a = i;
            while i < b.len() && b[i] != b'"' {
                if matches!(b[i], b'\\' | b'\n' | b'\r' | 0) {
                    return Err(bad());
                }
                i += 1
            }
            if i == b.len() {
                return Err(bad());
            }
            text = s[a..i].to_owned();
            i += 1;
        } else if matches!(b[i], b'{' | b'}') {
            i += 1;
            text = s[start..i].to_owned();
        } else {
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'{' | b'}') {
                if matches!(b[i], b'"' | b'[' | b']' | 0) {
                    return Err(bad());
                }
                i += 1
            }
            text = s[start..i].to_owned();
        }
        out.push(Token {
            text,
            start,
            end: i,
        });
        if out.len() > 100_000 {
            return Err(bad());
        }
    }
    Ok(out)
}
pub(crate) fn normalize_mount(s: &str) -> Option<String> {
    let s = s.replace('\\', "/");
    let s = s
        .strip_prefix("|gameinfo_path|")
        .map(|x| format!("citadel/{}", x.trim_start_matches('/')))
        .unwrap_or(s);
    if s.split('/').any(|v| v == ".." || v == ".") || s.starts_with('/') {
        return None;
    }
    Some(s.trim_end_matches('/').to_ascii_lowercase())
}
/// Resolve literal spelling without consulting the filesystem. Case folding is
/// comparison only, including the leading citadel component.
pub(crate) fn mount_path(c: &std::path::Path, mount: &str) -> Result<std::path::PathBuf, NativeError> {
    let spelling=mount.replace('\\', "/");
    let spelling=spelling.strip_prefix("|gameinfo_path|").map(|s|format!("citadel/{}",s.trim_start_matches('/'))).unwrap_or(spelling);
    let (head,relative)=spelling.split_once('/').filter(|(head,_)|head.eq_ignore_ascii_case("citadel")).ok_or_else(bad)?;
    let relative=relative.trim_end_matches('/');
    if relative.is_empty() || relative.split('/').any(|s|s.is_empty() || s=="." || s=="..") {return Err(bad());}
    Ok(c.parent().ok_or_else(bad)?.join(head).join(relative))
}
pub(crate) fn mount_directory(c: &std::path::Path, mount: &str) -> Result<std::path::PathBuf, NativeError> {
    let exact=mount_path(c,mount)?;
    let base=c.parent().ok_or_else(bad)?;
    let mut dir=base.to_path_buf();
    for part in exact.strip_prefix(base).map_err(|_|bad())?.components() {
        let part=part.as_os_str().to_str().ok_or_else(bad)?;
        // Once an exact parent is absent there can be no entries beneath it.
        // Check every existing parent, including files and symlinks as matches.
        if dir.try_exists()? {
            for entry in std::fs::read_dir(&dir)? {
                let name=entry?.file_name();
                if name.to_str().is_some_and(|s|s.eq_ignore_ascii_case(part) && s!=part) {
                    return Err(NativeError::new("ambiguousMount",format!("The folder {} differs only in capital letters from what the game is set to load. Rename it so the spelling matches, then try again.",name.to_string_lossy())));
                }
            }
        }
        dir.push(part);
        if dir.try_exists()? {crate::paths::reject_symlinks(&dir)?;}
    }
    Ok(dir)
}
/// Direct SearchPaths values from the same parser used for gameinfo edits.
pub(crate) fn mounts(s:&str)->Result<Vec<String>,NativeError>{
    let mut values=Vec::new();
    parse_inner(s,&HashSet::new(),None,&[],None,Some(&mut values))?;
    Ok(values)
}
pub fn replace(
    s: &str,
    known: &HashSet<String>,
    previous: Option<&str>,
    next: &str,
) -> Result<(String, Vec<String>), NativeError> {
    replace_inner(s, known, previous, &[next.to_owned()], None)
}
/// Exact native vector replacement. Only proven imported mounts and the entire
/// previous native sequence and exact original-manager addons line may be
/// removed; native spellings are canonical.
pub fn replace_many(s: &str, imported: &HashSet<String>, previous: &[String], next: &[String])
    -> Result<(String, Vec<String>), NativeError>
{
    let mut known = imported.clone();
    for mount in previous { known.insert(mount.clone()); }
    if previous.iter().collect::<HashSet<_>>().len()!=previous.len()
        || next.iter().collect::<HashSet<_>>().len()!=next.len()
        || next.iter().any(|m| normalize_mount(m).as_ref()!=Some(m) || !m.starts_with("citadel/deadlock_native/")) {
        return Err(bad());
    }
    replace_inner(s, &known, None, next, Some(previous))
}
fn replace_inner(s: &str, known: &HashSet<String>, previous: Option<&str>, next: &[String], exact_previous: Option<&[String]>)
    -> Result<(String, Vec<String>), NativeError>
{
    parse_inner(s,known,previous,next,exact_previous,None)
}
pub(crate) fn replace_many_unmanaged(s:&str,imported:&HashSet<String>,previous:&[String],next:&[String],unmanaged:&[String])->Result<(String,Vec<String>),NativeError>{
    if previous.iter().collect::<HashSet<_>>().len()!=previous.len()
        || next.iter().collect::<HashSet<_>>().len()!=next.len()
        || next.iter().chain(unmanaged).any(|m|normalize_mount(m).as_ref()!=Some(m) || !m.starts_with("citadel/deadlock_native/"))
        || unmanaged.iter().any(|m|previous.contains(m) || next.contains(m)) {return Err(bad());}
    let mut known=imported.clone();for m in previous {known.insert(m.clone());}
    parse_with_unmanaged(s,&known,None,next,Some(previous),None,unmanaged)
}
fn parse_inner(s:&str,known:&HashSet<String>,previous:Option<&str>,next:&[String],exact_previous:Option<&[String]>,values:Option<&mut Vec<String>>)
    ->Result<(String,Vec<String>),NativeError>{
    parse_with_unmanaged(s,known,previous,next,exact_previous,values,&[])
}
fn parse_with_unmanaged(s:&str,known:&HashSet<String>,previous:Option<&str>,next:&[String],exact_previous:Option<&[String]>,mut values:Option<&mut Vec<String>>,unmanaged:&[String])->Result<(String,Vec<String>),NativeError>{
    let t = tokens(s)?;
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0;
    let mut blocks = 0;
    let mut insert = None;
    let mut removals = Vec::new();
    let mut mounts = Vec::new();
    let mut seen = HashSet::new();
    let mut native = Vec::new();
    let mut original_addons_seen = false;
    while i < t.len() {
        if t[i].text == "}" {
            if stack.pop().is_none() {
                return Err(bad());
            }
            i += 1;
            continue;
        }
        if matches!(t[i].text.as_str(), "{" | "#base" | "#include") || i + 1 >= t.len() {
            return Err(bad());
        }
        let key = &t[i];
        let value = &t[i + 1];
        if value.text == "{" {
            if key.text.eq_ignore_ascii_case("SearchPaths") {
                blocks += 1;
                if stack.len() != 2
                    || !stack[0].eq_ignore_ascii_case("GameInfo")
                    || !stack[1].eq_ignore_ascii_case("FileSystem")
                {
                    return Err(bad());
                }
                let end = s[value.end..]
                    .find('\n')
                    .map(|n| value.end + n + 1)
                    .ok_or_else(bad)?;
                if !s[value.end..end].trim().is_empty() {
                    return Err(bad());
                }
                insert = Some(end);
            }
            stack.push(key.text.clone());
            if stack.len() > 64 {
                return Err(bad());
            }
            i += 2;
            continue;
        }
        if value.text == "}" {
            return Err(bad());
        }
        if stack
            .last()
            .is_some_and(|x| x.eq_ignore_ascii_case("SearchPaths"))
        {
            let norm = normalize_mount(&value.text).ok_or_else(bad)?;
            if let Some(values)=values.as_deref_mut() {
                values.push(value.text.clone());i+=2;continue;
            }
            if key.text=="Game" && unmanaged.contains(&value.text) {i+=2;continue;}
            let suspect = norm.starts_with("citadel/modlock/") || norm == "citadel/modlock" || norm
                .split('/')
                .any(|x| x.starts_with("addons") || x == "deadlock_native");
            let original_addons = key.text == "Game" && value.text == "citadel/addons";
            if original_addons {
                if original_addons_seen { return Err(bad()); }
                original_addons_seen = true;
            }
            if suspect || known.contains(&norm) {
                if !key.text.eq_ignore_ascii_case("Game")
                    || (!original_addons && !known.contains(&norm) && previous != Some(norm.as_str()))
                {
                    return Err(NativeError::new("unknownMount",format!("Unrecognized addon mount: {}. Preserve it and resolve ownership before Apply.",value.text)));
                }
                if exact_previous.is_some() {
                    if !seen.insert(norm.clone()) { return Err(bad()); }
                    if norm.split('/').any(|x| x == "deadlock_native") {
                        if value.text != norm { return Err(bad()); }
                        native.push(value.text.clone());
                    }
                }
                let start = s[..key.start].rfind('\n').map_or(0, |n| n + 1);
                let end = s[value.end..]
                    .find('\n')
                    .map_or(s.len(), |n| value.end + n + 1);
                if !s[start..key.start].trim().is_empty() {
                    return Err(bad());
                }
                let suffix = s[value.end..end].trim();
                if !suffix.is_empty() && !suffix.starts_with("//") {
                    return Err(bad());
                }
                // New vector transforms retain comments attached to a replaced
                // line as their original indentation/suffix bytes. Historical
                // replace keeps its established whole-line behavior.
                let replacement = if exact_previous.is_some() && suffix.starts_with("//") {
                    format!("{}{}", &s[start..key.start], &s[value.end..end])
                } else {String::new()};
                removals.push((start, end, replacement));
                mounts.push(value.text.clone());
            }
        }
        i += 2;
    }
    if !stack.is_empty() || blocks != 1 {
        return Err(bad());
    }
    if exact_previous.is_some_and(|expected| native != expected) { return Err(bad()); }
    let at = insert.ok_or_else(bad)?;
    let nl = if s.contains("\r\n") { "\r\n" } else { "\n" };
    let mut edits: Vec<(usize, usize, String)> = removals;
    edits.push((at, at, next.iter().map(|mount| format!("\t\t\tGame\t\"{mount}\"{nl}")).collect()));
    edits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = s.to_owned();
    for (a, b, replacement) in edits {
        out.replace_range(a..b, &replacement)
    }
    Ok((out, mounts))
}

#[cfg(test)]
mod scoped_mount_tests {
    use super::*;
    #[test]
    fn modlock_mount_is_unknown() {
        let source="GameInfo\n{\nFileSystem\n{\nSearchPaths\n{\nGame citadel/addons\nGame citadel/modlock/foreign/shard01\nGame citadel\n}\n}\n}\n";
        let known=HashSet::from(["citadel/addons".to_owned()]);
        assert_eq!(replace(source,&known,None,"citadel/deadlock_native/new").unwrap_err().kind,"unknownMount");
        assert_eq!(replace_many(source,&known,&[],&["citadel/deadlock_native/new/shard01".into()]).unwrap_err().kind,"unknownMount");
    }
}

#[cfg(test)]
mod edge_mount_tests {
    use super::*;
    #[test]
    fn nested_modlock_component_keeps_historical_parse() {
        let source="GameInfo\n{\nFileSystem\n{\nSearchPaths\n{\nGame citadel/addons\nGame citadel/custom/modlock/assets\nGame citadel\n}\n}\n}\n";
        let known=HashSet::from(["citadel/addons".to_owned()]);
        let (flat,_)=replace(source,&known,None,"citadel/deadlock_native/new").unwrap();
        let (sharded,_)=replace_many(source,&known,&[],&["citadel/deadlock_native/new/shard01".into()]).unwrap();
        for result in [flat,sharded] {assert!(result.contains("Game citadel/custom/modlock/assets\n"));}
    }
}
