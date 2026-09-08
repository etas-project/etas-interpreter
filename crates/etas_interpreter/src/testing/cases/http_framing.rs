use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn checked_http_prefix_codecs_preserve_boundaries_limits_and_u64_size() {
    let checked = checked_project(
        r#"
module app.main;
import std.codec.text.utf8_encode;
import std.http.codec.{Complete, Malformed, NeedMore, LimitExceeded, InvalidChunkExtension,
    decode_response_head_prefix, decode_chunk_size_line_prefix};

flow main() -> bool {
    let head_ok = match decode_response_head_prefix(
        utf8_encode("HTTP/1.1 200 OK\r\n\r\nbody"), 19) {
        Complete(head, consumed) => head.status == 200 && consumed == 19,
        _ => false
    };
    let chunk_ok = match decode_chunk_size_line_prefix(
        utf8_encode("100000000\r\nDATA"), 11) {
        Complete(size, consumed) => size == 4294967296 && consumed == 11,
        _ => false
    };
    let partial_ok = match decode_chunk_size_line_prefix(utf8_encode("a;name="), 64) {
        NeedMore => true,
        _ => false
    };
    let limit_ok = match decode_response_head_prefix(utf8_encode("HTTP/1.1 200 OK\r\n\r\n"), 18) {
        Malformed(error) => match error.kind { LimitExceeded => error.offset == 18, _ => false },
        _ => false
    };
    let invalid_ok = match decode_chunk_size_line_prefix(utf8_encode("a;name=\r\n"), 64) {
        Malformed(error) => match error.kind { InvalidChunkExtension => true, _ => false },
        _ => false
    };
    return head_ok && chunk_ok && partial_ok && limit_ok && invalid_ok;
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
}
