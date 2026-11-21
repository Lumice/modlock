use keyvalues_parser::parse;

const ONE_BASE_PER_LINE: &str = r##"
#base"foo.vdf"#base"bar.vdf"
key val
"##;

#[test]
#[should_panic] // FIXME(cosmic): well it shouldn't parse right, but it currently does...
fn one_base_per_line() {
    parse(ONE_BASE_PER_LINE).unwrap_err();
}
