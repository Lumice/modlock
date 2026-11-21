mod support;
use modlock_helper::protocol::serve;
use json::{object, JsonValue};
use std::io::Cursor;

fn responses(input: &[u8]) -> Vec<JsonValue> {
    let mut output = Vec::new();
    serve(Cursor::new(input), &mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    output
        .lines()
        .map(|line| json::parse(line).expect("one JSON object per response line"))
        .collect()
}

#[test]
fn handshake_advertises_implemented_native_capabilities_and_echoes_integer_id() {
    let result = responses(b"{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"hello\"}\n");
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0],
        object! {jsonrpc:"2.0",id:42,result:object!{protocolVersion:1,readOnly:false,capabilities:JsonValue::from(modlock_helper::native::CAPABILITIES.to_vec())}}
    );
    let advertised = &result[0]["result"]["capabilities"];
    for core in ["library.snapshot","native.snapshot","native.importPreview","native.importCommit","native.setEnabled","native.profile","native.applyPreview","native.apply","native.restore","native.recover","native.local.prepare","native.local.commit"] {
        assert!(advertised.members().any(|c| c == core), "{core}");
    }
}

#[test]
fn snapshot_preserves_escaped_unicode_newlines_and_omits_secrets() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/upstream-state-v33.json");
    let request = object!{jsonrpc:"2.0",id:-3,method:"library.snapshot",params:object!{statePath:path.to_str().unwrap()}}.dump()+"\n";
    let r = responses(request.as_bytes());
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["id"], -3);
    assert_eq!(
        r[0]["result"]["mods"][0]["title"],
        "Ivy — Täht 🌿\nSecond line"
    );
    assert!(!r[0].dump().contains("SYNTHETIC_SECRET_DO_NOT_EXPOSE"));
}

#[test]
fn unknown_methods_fail_and_notifications_never_reply() {
    let r = responses(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"mods.enable\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"hello\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"mods.enable\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"library.snapshot\",\"params\":false}\n");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["error"]["code"], -32601);
}

#[test]
fn parse_invalid_request_and_params_errors_recover_on_next_line() {
    let r = responses(b"bad JSON\n[]\n{\"id\":1,\"method\":\"hello\"}\n{\"jsonrpc\":\"2.0\",\"id\":\"bad\",\"method\":\"hello\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"library.snapshot\",\"params\":false}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"library.snapshot\",\"params\":{\"statePath\":1}}\n{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"library.snapshot\",\"params\":{\"profileId\":false}}\n{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"hello\"}\n");
    assert_eq!(r.len(), 8);
    for (index, code) in [-32700, -32600, -32600, -32600, -32602, -32602, -32602]
        .into_iter()
        .enumerate()
    {
        assert_eq!(r[index]["error"]["code"], code);
    }
    assert!(r[0]["id"].is_null());
    assert!(r[3]["id"].is_null());
    assert_eq!(r[7]["result"]["readOnly"], false);
}

#[test]
fn missing_explicit_source_reports_error_without_fallback() {
    let r = responses(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"library.snapshot\",\"params\":{\"statePath\":\"/this-synthetic-source-does-not-exist/state.json\"}}\n");
    assert_eq!(r[0]["id"], 7);
    assert_eq!(r[0]["error"]["code"], -32010);
    assert_eq!(r[0]["error"]["data"]["kind"], "sourceNotFound");
}

#[test]
fn eof_and_final_line_without_newline_exit_cleanly() {
    assert!(responses(b"").is_empty());
    let r = responses(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"hello\"}");
    assert_eq!(r.len(), 1);
}

#[test]
fn oversized_line_is_drained_with_bounded_reads_and_next_request_works() {
    // An oversized request is discarded before the following line is parsed.
    let mut input = vec![b'x'; 1024 * 1024];
    input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"hello\"}\n");
    let r = responses(&input);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["error"]["code"], -32600);
    assert!(r[0]["id"].is_null());
    assert_eq!(r[1]["result"]["readOnly"], false);
}

#[test]
fn invalid_utf8_is_a_parse_error_and_does_not_break_later_requests() {
    let r = responses(b"\xff\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"hello\"}\n");
    assert_eq!(r[0]["error"]["code"], -32700);
    assert_eq!(r[1]["id"], 1);
}

#[test]
fn oversized_serialized_snapshot_returns_small_error_without_partial_json() {
    let path = std::env::temp_dir().join(format!(
        "modlock-helper-response-limit-{}.json",
        std::process::id()
    ));
    let mut fixture =
        json::parse(include_str!("../../tests/fixtures/upstream-state-v33.json")).unwrap();
    // JSON escaping expands this six MiB title to thirty-six MiB on the wire.
    fixture["local-config"]["state"]["localMods"][0]["name"] =
        "\u{1}".repeat(6 * 1024 * 1024).into();
    std::fs::write(&path, fixture.dump()).unwrap();
    let request = object!{jsonrpc:"2.0",id:91,method:"library.snapshot",params:object!{statePath:path.to_str().unwrap()}}.dump()+"\n";
    let r = responses(request.as_bytes());
    std::fs::remove_file(path).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["id"], 91);
    assert_eq!(r[0]["error"]["code"], -32603);
    assert_eq!(r[0]["error"]["data"]["kind"], "responseTooLarge");
    assert!(r[0].dump().len() < 1024);
}

#[test]
fn integer_ids_are_echoed_without_loss_at_signed_boundaries() {
    let r = responses(b"{\"jsonrpc\":\"2.0\",\"id\":-9223372036854775808,\"method\":\"hello\"}\n{\"jsonrpc\":\"2.0\",\"id\":9223372036854775807,\"method\":\"hello\"}\n");
    assert_eq!(r.len(), 2);
    assert!(r[0].dump().contains("\"id\":-9223372036854775808"));
    assert!(r[1].dump().contains("\"id\":9223372036854775807"));
}

#[test]
fn integral_exponent_and_decimal_request_ids_are_accepted() {
    for (input, expected) in [
        ("1e2", "100"),
        ("100.0", "100"),
        ("1.2e1", "12"),
        ("10e-1", "1"),
        ("-1e2", "-100"),
        ("0e100", "0"),
    ] {
        let input = format!("{{\"jsonrpc\":\"2.0\",\"id\":{input},\"method\":\"hello\"}}\n");
        let r = responses(input.as_bytes());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["result"]["readOnly"], false, "{input}");
        assert_eq!(r[0]["id"], json::parse(expected).unwrap(), "{input}");
    }
}

#[test]
fn fractional_and_out_of_signed_range_request_ids_are_invalid() {
    for id in [
        "1.5",
        "1.25",
        "1e-1",
        "1e20",
        "-1e20",
        "100000000000000000000",
        "9223372036854775808",
        "-9223372036854775809",
        "18446744073709551615",
    ] {
        let input = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"hello\"}}\n");
        let r = responses(input.as_bytes());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["error"]["code"], -32600, "{id}");
        assert!(r[0]["id"].is_null(), "{id}");
    }
}

#[test]
fn native_parameter_errors_and_apply_required_parameters_are_explicit() {
    let r=responses(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"native.snapshot\",\"params\":false}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"native.apply\",\"params\":{}}\n");
    assert_eq!(r[0]["error"]["code"], -32602);
    assert_eq!(r[1]["error"]["code"], -32602);
}

#[test]
fn local_notifications_never_create_job_storage_or_mutate_library() {
    let dir = std::env::temp_dir().join(format!(
        "deadlock-local-notification-{}",
        std::process::id()
    ));
    let requests=format!("{}\n{}\n",object!{jsonrpc:"2.0",method:"native.local.prepare",params:object!{dataDir:dir.to_str().unwrap(),jobId:"a".repeat(32),profileId:"default",expectedRevision:"1",roots:json::array!["/not-used.vpk"]}}.dump(),object!{jsonrpc:"2.0",id:55,method:"hello"}.dump());
    let r = responses(requests.as_bytes());
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["id"], 55);
    assert!(!dir.exists());
}
#[test]
fn output_write_and_flush_failures_still_drop_local_service() {
    use std::io::{self, Write};
    struct Fail {
        flush: bool,
    }
    impl Write for Fail {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if self.flush {
                Ok(b.len())
            } else {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
    for flush in [false, true] {
        let f = support::Fixture::new();
        let mut s = modlock_helper::native::NativeService::new();
        support::import(&mut s, &f);
        drop(s);
        let source = f.root.join("local.vpk");
        std::fs::write(&source, support::vpk("a/b.vmdl_c", 1)).unwrap();
        let params = object! {dataDir:f.data.to_str().unwrap(),jobId:"b".repeat(32),profileId:"default",expectedRevision:"1",roots:json::array![source.to_str().unwrap()]};
        let request =
            object! {jsonrpc:"2.0",id:1,method:"native.local.prepare",params:params}.dump() + "\n";
        let result = serve(Cursor::new(request), Fail { flush });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        let mut s = modlock_helper::native::NativeService::new();
        let status = s
            .dispatch(
                "native.local.status",
                &object! {dataDir:f.data.to_str().unwrap(),jobId:"b".repeat(32)},
            )
            .unwrap();
        assert_eq!(status["phase"], "interrupted");
    }
}

#[test]
fn output_failure_after_mutation_retains_durable_success() {
    use std::io::{self, Write};
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let f = support::Fixture::new();
    let mut service = modlock_helper::native::NativeService::new();
    support::import(&mut service, &f);
    drop(service);
    let mut params = support::mutation(&f, "1", "broken-output-writer");
    params["action"] = "rename".into();
    params["profileId"] = "default".into();
    params["name"] = "Saved before output".into();
    let request = object! {jsonrpc:"2.0",id:1,method:"native.profile",params:params}.dump() + "\n";
    assert_eq!(
        serve(Cursor::new(request), Broken).unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    let mut service = modlock_helper::native::NativeService::new();
    let outcome = service
        .dispatch(
            "native.operation",
            &object! {dataDir:f.data.to_str().unwrap(),operationId:"broken-output-writer"},
        )
        .unwrap();
    assert_eq!(outcome["status"], "succeeded");
    assert_eq!(outcome["result"]["revision"], "2");
}
