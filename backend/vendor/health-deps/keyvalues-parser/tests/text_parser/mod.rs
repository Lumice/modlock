use insta::{assert_debug_snapshot, assert_snapshot};

use std::{fs, path::Path};

use keyvalues_parser::{parse, Parser};

fn read_asset_file(file_name: &str) -> String {
    fs::read_to_string(Path::new("tests").join("assets").join(file_name)).unwrap()
}

// Snapshots both parsing and re-rendering the text from a file
fn snapshot_test_parse_and_render(snapshot_name_base: &str, file_name: &str) {
    let vdf_text = read_asset_file(file_name);
    let vdf = parse(&vdf_text).unwrap().into_vdf();
    assert_debug_snapshot!(format!("parsed-{snapshot_name_base}"), vdf);
    assert_snapshot!(format!("rendered-{snapshot_name_base}"), vdf);
}

fn snapshot_test_raw_parse_render(snapshot_name_base: &str, file_name: &str) {
    let vdf_text = read_asset_file(file_name);
    let vdf = Parser::new()
        .literal_special_chars(true)
        .parse(&vdf_text)
        .unwrap()
        .into_vdf();
    assert_debug_snapshot!(format!("parsed-{snapshot_name_base}"), vdf);

    let mut rendered = String::new();
    vdf.render_raw(&mut rendered).unwrap();
    assert_snapshot!(format!("rendered-{snapshot_name_base}"), rendered);
}

// Snapshots both parsing and re-rendering the text from a file
fn snapshot_test_partial_parse_and_render(snapshot_name_base: &str, file_name: &str) {
    let vdf_text = read_asset_file(file_name);
    let vdf = Parser::new()
        .literal_special_chars(true)
        .parse(&vdf_text)
        .unwrap();
    assert_debug_snapshot!(format!("parsed-{snapshot_name_base}"), vdf);
    assert_snapshot!(format!("rendered-{snapshot_name_base}"), vdf);
}

fn snapshot_test_partial_raw_parse_render(snapshot_name_base: &str, file_name: &str) {
    let vdf_text = read_asset_file(file_name);
    let vdf = Parser::new()
        .literal_special_chars(true)
        .parse(&vdf_text)
        .unwrap();
    assert_debug_snapshot!(format!("parsed-{snapshot_name_base}"), vdf);

    let mut rendered = String::new();
    vdf.render_raw(&mut rendered).unwrap();
    assert_snapshot!(format!("rendered-{snapshot_name_base}"), rendered);
}

// Generates tests where the `name`s indicate the unit test name and the file without an extension
macro_rules! parse_test_generator {
    ( $test_type:ident, $( $name:ident ),* ) => {
        $(
            #[test]
            fn $name() {
                let name_str = stringify!($name);
                ($test_type)(name_str, &format!("{name_str}.vdf"));
            }
        )*
    }
}

parse_test_generator!(
    snapshot_test_parse_and_render,
    basic,
    comments,
    compact,
    unquoted_strings,
    special_characters,
    null_byte
);

parse_test_generator!(snapshot_test_raw_parse_render, raw_strings);

parse_test_generator!(
    snapshot_test_partial_parse_and_render,
    base_multiple,
    base_quoted,
    base_unquoted
);

parse_test_generator!(
    snapshot_test_partial_raw_parse_render,
    base_multiple_raw_strings
);
