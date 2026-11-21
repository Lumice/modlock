//! Bounded allowlisted description projection. No provider HTML crosses this boundary.
use json::{object, JsonValue};
use regex::Regex;
use std::sync::LazyLock;
static TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<!--.*?-->|<\s*/?\s*[a-zA-Z][^<>"']*(?:(?:"[^"]*"|'[^']*')[^<>"']*)*>|[^<]+|<"#,
    )
    .unwrap()
});
static ATTRS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([a-zA-Z][a-zA-Z0-9_-]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap()
});
fn decode(s: &str) -> String {
    static ENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"&(#x[0-9a-fA-F]{1,6}|#[0-9]{1,7}|amp|lt|gt|quot|apos|nbsp);").unwrap()
    });
    ENT.replace_all(s, |c: &regex::Captures| match &c[1] {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        "nbsp" => " ".into(),
        n => {
            let v = if let Some(h) = n.strip_prefix("#x") {
                u32::from_str_radix(h, 16).ok()
            } else {
                n[1..].parse().ok()
            };
            v.and_then(char::from_u32)
                .filter(|c| !c.is_control())
                .map(|c| c.to_string())
                .unwrap_or_default()
        }
    })
    .into_owned()
}
fn url(s: &str) -> Option<String> {
    let s = decode(s);
    let s = if s.starts_with('/') && !s.starts_with("//") {
        format!("https://gamebanana.com{s}")
    } else {
        s
    };
    let https = if let Some(t) = s.strip_prefix("http://") {
        format!("https://{t}")
    } else {
        s.clone()
    };
    (s.len() <= 2048 && super::provider::https(&https) && !s.contains(['<', '>', '"', '\'']))
        .then_some(s)
}
fn color(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    if s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(s);
    }
    if s.len() == 4 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(format!(
            "#{}",
            s[1..].chars().flat_map(|c| [c, c]).collect::<String>()
        ));
    }
    if let Some(v) = s.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        let v: Option<Vec<u8>> = v.split(',').map(|v| v.trim().parse().ok()).collect();
        if let Some(v) = v {
            if v.len() == 3 {
                return Some(format!("#{:02x}{:02x}{:02x}", v[0], v[1], v[2]));
            }
        }
    }
    match s.as_str() {
        "red" => Some("#ff0000".into()),
        "blue" => Some("#0000ff".into()),
        "green" => Some("#008000".into()),
        "white" => Some("#ffffff".into()),
        "black" => Some("#000000".into()),
        "yellow" => Some("#ffff00".into()),
        "orange" => Some("#ffa500".into()),
        "purple" => Some("#800080".into()),
        _ => None,
    }
}
struct Output {
    blocks: Vec<JsonValue>,
    runs: Vec<JsonValue>,
    kind: String,
    level: u8,
    list: Option<(bool, Vec<JsonValue>)>,
    count: usize,
    bytes: usize,
    images: usize,
}
impl Output {
    fn flush(&mut self) {
        if self.runs.is_empty() {
            return;
        }
        let runs = JsonValue::from(std::mem::take(&mut self.runs));
        if let Some((_, items)) = &mut self.list {
            items.push(runs)
        } else if self.blocks.len() < 256 {
            let mut b = object! {type:self.kind.clone(),runs:runs};
            if self.kind == "heading" {
                b["level"] = self.level.into()
            }
            self.blocks.push(b);
        }
    }
    fn end_list(&mut self) {
        self.flush();
        if let Some((ordered, items)) = self.list.take() {
            if !items.is_empty() && self.blocks.len() < 256 {
                self.blocks
                    .push(object! {type:"list",ordered:ordered,items:items});
            }
        }
    }
    fn text(&mut self, text: &str, style: &JsonValue) {
        if self.count >= 2048 || self.bytes >= 65536 {
            return;
        }
        let text = decode(text).replace(['\r', '\n', '\t'], " ");
        let mut end = text.len().min(65536 - self.bytes);
        while !text.is_char_boundary(end) {
            end -= 1
        }
        if end == 0 {
            return;
        }
        let mut r = style.clone();
        r["text"] = text[..end].into();
        self.bytes += end;
        self.count += 1;
        self.runs.push(r);
    }
}
pub fn project(markup: &str) -> JsonValue {
    // Fail closed to the existing plain fallback for oversized input.
    if markup.len() > 256 * 1024 {
        return JsonValue::new_array();
    }
    let mut out = Output {
        blocks: vec![],
        runs: vec![],
        kind: "paragraph".into(),
        level: 1,
        list: None,
        count: 0,
        bytes: 0,
        images: 0,
    };
    let mut stack: Vec<(String, JsonValue)> = vec![];
    let mut style = object! {};
    let mut skip: Option<String> = None;
    for token in TOKENS.find_iter(markup) {
        if out.blocks.len() >= 256 || out.count >= 2048 || out.bytes >= 65536 {
            break;
        }
        let t = token.as_str();
        if !t.starts_with('<') || t == "<" {
            if skip.is_none() {
                out.text(t, &style)
            }
            continue;
        }
        if t.starts_with("<!--") {
            continue;
        }
        let inner = t[1..t.len() - 1].trim();
        let closing = inner.starts_with('/');
        let inner = inner.trim_start_matches('/').trim();
        let name = inner
            .split(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if let Some(s) = &skip {
            if closing && s == &name {
                skip = None
            }
            continue;
        }
        if [
            "script", "style", "iframe", "form", "object", "svg", "math", "template",
        ]
        .contains(&name.as_str())
        {
            if !closing {
                skip = Some(name)
            }
            continue;
        }
        if closing {
            if matches!(name.as_str(), "ul" | "ol") {
                out.end_list()
            }
            if matches!(
                name.as_str(),
                "p" | "div" | "li" | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            ) {
                out.flush();
                out.kind = "paragraph".into()
            }
            if let Some(i) = stack.iter().rposition(|(n, _)| n == &name) {
                style = stack[i].1.clone();
                stack.truncate(i)
            }
            continue;
        }
        let attrs: Vec<(String, String)> = ATTRS
            .captures_iter(inner)
            .take(32)
            .map(|c| {
                (
                    c[1].to_ascii_lowercase(),
                    decode(c.get(2).or(c.get(3)).or(c.get(4)).unwrap().as_str()),
                )
            })
            .collect();
        let attr = |key: &str| {
            attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        match name.as_str() {
            "br" => {
                out.text("\n", &style);
                if let Some(r) = out.runs.last_mut() {
                    r["text"] = "\n".into()
                }
                continue;
            }
            "ul" | "ol" => {
                out.end_list();
                out.list = Some((name == "ol", vec![]));
            }
            "li" => out.flush(),
            "p" | "div" | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                out.flush();
                out.kind = if name == "blockquote" {
                    "quote"
                } else if name.starts_with('h') {
                    "heading"
                } else {
                    "paragraph"
                }
                .into();
                out.level = name[1..].parse::<u8>().unwrap_or(1).min(3);
            }
            "hr" => {
                out.end_list();
                out.blocks.push(object! {type:"rule"});
                continue;
            }
            "img" => {
                if out.images < 16 {
                    if let Some(u) = url(attr("src")).filter(|u| {
                        u.starts_with("https://images.gamebanana.com/")
                            || u.starts_with("https://puu.sh/")
                    }) {
                        out.end_list();
                        let mut b = object! {type:"image",url:u,alt:attr("alt").chars().take(1024).collect::<String>()};
                        for key in ["width", "height"] {
                            if let Ok(n) = attr(key).parse::<u32>() {
                                if (1..=8192).contains(&n) {
                                    b[key] = n.into()
                                }
                            }
                        }
                        out.blocks.push(b);
                        out.images += 1;
                    }
                }
                continue;
            }
            _ => {}
        }
        if stack.len() >= 64 {
            return JsonValue::new_array();
        }
        stack.push((name.clone(), style.clone()));
        match name.as_str() {
            "b" | "strong" => style["bold"] = true.into(),
            "i" | "em" => style["italic"] = true.into(),
            "u" => style["underline"] = true.into(),
            "s" | "del" | "strike" => style["strike"] = true.into(),
            "code" | "pre" => style["code"] = true.into(),
            "a" => {
                style.remove("link");
                if let Some(u) = url(attr("href")) {
                    style["link"] = u.into()
                }
            }
            _ => {}
        }
        let css = attr("style")
            .split(';')
            .filter_map(|d| d.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("color"))
            .and_then(|(_, v)| color(v));
        let class = attr("class").split_whitespace().find_map(|c| match c {
            "RedColor" => color("red"),
            "BlueColor" => color("blue"),
            "GreenColor" => color("green"),
            "YellowColor" => color("yellow"),
            "OrangeColor" => color("orange"),
            "PurpleColor" => color("purple"),
            _ => None,
        });
        if let Some(c) = css
            .or_else(|| {
                if name == "font" {
                    color(attr("color"))
                } else {
                    None
                }
            })
            .or(class)
        {
            style["color"] = c.into()
        }
        if inner.ends_with('/') {
            if let Some((_, previous)) = stack.pop() {
                style = previous
            }
        }
    }
    out.end_list();
    out.blocks.truncate(256);
    let result: JsonValue = out.blocks.into();
    if result.dump().len() > 256 * 1024 {
        JsonValue::new_array()
    } else {
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rich_description_malicious_and_limits() {
        let v = project(
            r#"<script>bad</script><style>bad</style><iframe>bad</iframe><form>bad</form><p onclick="bad"><a href="javascript:alert(1)">safe</a>&lt;img onerror=&quot;bad&quot;&gt;</p><img src="data:image/png;base64,bad" onerror="bad"><img src="https://evil.test/banner"><img src="https://images.gamebanana.com.evil.test/a"><span style="color:expression(bad)">plain</span>"#,
        );
        let s = v.dump();
        assert!(!s.contains("javascript:"));
        assert!(!s.contains("iframe"));
        assert!(!v.members().any(|b| b["type"] == "image"));
        assert!(v[0]["runs"]
            .members()
            .any(|r| r["text"].as_str().unwrap_or("").contains("<img onerror=")));
        assert_eq!(project(&"x".repeat(262145)).len(), 0);
        assert!(project(&"<p>x</p>".repeat(3000)).len() <= 256);
        assert!(project(&"<img src='https://images.gamebanana.com/a'>".repeat(40)).len() <= 16);
        assert!(project(&"<b>x</b>".repeat(3000))[0]["runs"].len() <= 2048);
        assert_eq!(project(&"<span>".repeat(65)).len(), 0);
    }
    #[test]
    fn rich_description_synthetic_samples() {
        let list = project(include_str!(
            "../../tests/fixtures/catalog/styled-list.html"
        ));
        assert!(list.dump().contains("#ff0000"));
        assert!(list.dump().contains("italic"));
        assert!(list.members().any(|b| b["type"] == "list"));
        assert!(list.dump().contains("https://gamebanana.com/mods/900000004"));
        let media = project(include_str!(
            "../../tests/fixtures/catalog/inline-media.html"
        ));
        assert!(media.dump().contains("#ff0000"));
        assert!(media.members().any(|b| b["type"] == "rule"));
        assert!(media.members().any(|b| b["type"] == "image"
            && b["url"] == "https://puu.sh/modlock-fixture/planter.png"
            && b["width"] == 480
            && b["height"] == 320));
        assert!(!media.dump().contains("Ignored embedded player"));
        let v=project("<p><font color='#aBc'>hex</font><span style='color:rgb(12, 34, 56)'>rgb</span><a href='http://example.org/'>http</a></p>");
        assert_eq!(v[0]["runs"][0]["color"], "#aabbcc");
        assert_eq!(v[0]["runs"][1]["color"], "#0c2238");
        assert_eq!(v[0]["runs"][2]["link"], "http://example.org/");
    }
    #[test]
    fn rich_description_formats() {
        let v=project("<h1>Title</h1><p><b>Bold</b> <i>italic</i><span class='RedColor'>red</span></p><ol><li>One</li><li>Two</li></ol><hr><blockquote>Quote</blockquote><img src='https://images.gamebanana.com/banner.png'>");
        assert_eq!(v[0]["type"], "heading");
        assert_eq!(v[1]["runs"][0]["bold"], true);
        assert!(v.dump().contains("#ff0000"));
        assert!(v
            .members()
            .any(|b| b["type"] == "list" && b["items"].len() == 2));
        assert!(v.members().any(|b| b["type"] == "image"));
    }
}
