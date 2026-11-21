pub fn path_key(p: &[String]) -> String {
    p.join("/").to_ascii_lowercase()
}
pub fn values_equal(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b)
}
fn normalize(s: &str) -> String {
    match s.to_ascii_lowercase().as_str() {
        "true" => "1".into(),
        "false" => "0".into(),
        _ => s
            .parse::<f64>()
            .map(|n| n.to_string())
            .unwrap_or_else(|_| s.to_string()),
    }
}
