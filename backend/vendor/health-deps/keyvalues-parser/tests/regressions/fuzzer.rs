use keyvalues_parser::{parse, Vdf};
use pretty_assertions::assert_eq;

// Mimics the behavior of the parse fuzzer test for regressions testing
fn parse_valid(contents: &str) {
    let parsed: Vdf = parse(contents).expect("Input has to be valid here").into();
    let vdf_text = parsed.to_string();
    let reparsed: Vdf = parse(&vdf_text).unwrap().into();
    assert_eq!(parsed, reparsed);
}

// Checks that we return an error instead of panicking or hanging
fn parse_invalid(contents: &str) {
    parse(contents).unwrap_err();
}

macro_rules! gen_fuzzer_tests {
    ( $test_fn:ident, $( ( $name:ident, $input:expr ) ),* $(,)? ) => {
        $(
            #[test]
            fn $name() {
                let contents = $input;
                $test_fn(contents);
            }
        )*
    };
}

mod valid {
    use super::*;

    gen_fuzzer_tests!(
        parse_valid,
        (unqoted_backslash_key, r#"\ """#),
        (escaped_chars, r#""" "\r\\\n\t\"""#),
    );
}

mod invalid {
    use super::*;

    gen_fuzzer_tests!(
        parse_invalid,
        (empty, ""),
        (partial_map, "a{\n\"\""),
        (macrolike_key_then_map, "#basefoo{}"),
        (macrolike_key_then_str, "#base no_vdf"),
        (trailing_bytes, "foo {}\n\ntrailing bytes"),
    );
}
