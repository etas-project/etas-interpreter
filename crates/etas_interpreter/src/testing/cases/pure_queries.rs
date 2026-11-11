use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn checked_char_slice_resumes_after_copying_across_work_quanta() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.{get_char_slice, len};

flow main() -> bool {
    var text = "x";
    var index = 0;
    while index < 17 limit Iterations(17) {
        text = text + text;
        index = index + 1;
    }
    return match get_char_slice(text, Range.closed(0, 70000)) {
        Some(value) => len(value) == 70001,
        None() => false,
    };
}
"#,
    );
    let host = FakeHost::new(HostServiceAvailability::default());
    let run = Interpreter.run_checked(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .expect("multi-quantum char slice must make progress")
        .unwrap();

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn segmented_text_keys_preserve_membership_order_and_checkpoint_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> bool {
    let prefix = "中😀";
    let key = prefix + "é";
    let map = {key => 7};
    let set = #{key, "中😀é"};
    let ordered = OrderedMap.new<string, i32>().insert(key, 9).insert("a", 1);
    checkpoint("segmented-text-keys");
    let other = "中" + "😀é";
    var count = 0;
    for item in set limit Iterations(4) {
        if item != other { return false; }
        count = count + 1;
    }
    return key == other && key != "中😀é"
        && map.get(other) == Some(7) && map.get("missing") == None
        && count == 1
        && ordered.get(other) == Some(9) && ordered.get("a") == Some(1);
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&[], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for checkpoint in [saved, &decoded] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn segmented_text_transforms_preserve_unicode_fragments_and_file_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.{trim, lines, split, join};
import std.runtime.checkpoint;
flow main() -> bool {
    let prefix = "  A\r";
    let source = prefix + "\n中😀\n\ré  ";
    let alias = source;
    let trimmed = trim(source);
    let parts = lines(trimmed);
    var line_count = 0;
    var lines_ok = true;
    for part in parts limit Iterations(4) {
        if line_count == 0 && part != "A" { lines_ok = false; }
        if line_count == 1 && part != "中😀" { lines_ok = false; }
        if line_count == 2 && part != "\ré" { lines_ok = false; }
        line_count = line_count + 1;
    }
    let split_prefix = "中😀:";
    let separator_prefix = ":";
    let separated = split(split_prefix + ":tail::", separator_prefix + ":");
    let joined = join(separated, "-" + ":");
    checkpoint("segmented-text-transform");
    return lines_ok && line_count == 3
        && separated == ["中😀", "tail", ""]
        && joined == "中😀-:tail-:"
        && alias == "  A\r\n中😀\n\ré  ";
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for checkpoint in [saved, &decoded] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
    }
    assert_eq!(
        checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap(),
        artifact
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checked_composite_text_queries_preserve_values_and_file_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.collections.{len, is_empty};
import std.text.{len as text_len, char_count, char_get, char_slice, get_char_slice, contains, starts_with, ends_with, strip_prefix, strip_suffix, eq_ignore_ascii_case, parse_i32};
import std.runtime.checkpoint;
flow main() -> bool {
    var text = "start";
    for _ in Range.closed(1, 1000) limit Iterations(1000) { text = text + "中😀é"; }
    let previous = text;
    text = text + "end";
    let numeric_prefix = "000";
    let parsed = match parse_i32(numeric_prefix + "2147483647") { Ok(value) => value == 2147483647, Err(_) => false };
    let prefix_removed = match strip_prefix(text, "start") { Some(value) => text_len(value) == 10003 && ends_with(value, "end"), None => false };
    let suffix_removed = match strip_suffix(text, "end") { Some(value) => text_len(value) == 10005 && starts_with(value, "start"), None => false };
    let correct = len(text) == 10008 && text_len(text) == 10008 && char_count(text) == 4008 && !is_empty(text)
        && starts_with(text, "start中") && ends_with(text, "éend")
        && contains(text, "éend") && !contains(text, "absent")
        && char_get(text, 5) == Some('中') && char_get(text, 6) == Some('😀')
        && char_get(text, 7) == Some('e') && char_get(text, 8) == Some('́')
        && char_slice(text, Range.closed(5, 6)) == "中😀"
        && get_char_slice(text, Range.closed(10000, 10001)) == None
        && prefix_removed && suffix_removed
        && eq_ignore_ascii_case(text, previous + "END") && parsed;
    checkpoint("composite-queries");
    text = text + "!";
    return correct && len(previous) == 10005 && len(text) == 10009;
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for snapshot in [saved, &decoded] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, snapshot, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
    }
    assert_eq!(
        checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap(),
        artifact
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checked_text_parser_returns_a_checked_error_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.{parse_i32, TextError};
import std.runtime.checkpoint;
flow main() -> Result<i32, TextError> {
    let error = parse_i32("not-an-integer");
    checkpoint("parse-error");
    return error;
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let InterpValue::Variant { name, fields } = result.value().unwrap() else {
        panic!("checked Result error")
    };
    assert_eq!(name.as_str(), "Err");
    let [InterpValue::Nominal { ty, value }] = fields.as_ref() else {
        panic!("checked nominal TextError")
    };
    assert_eq!(
        etas_types::ty::display_type(&checked.type_store, *ty),
        "std.text.TextError"
    );
    assert_eq!(
        value.as_ref(),
        &InterpValue::Variant {
            name: "ParseFailure".into(),
            fields: Vec::new().into(),
        }
    );
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for snapshot in [saved, &decoded] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, snapshot, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checked_text_numeric_family_preserves_every_concrete_width() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.{parse_i8, parse_i16, parse_i32, parse_i64, parse_i128, parse_isize, parse_u8, parse_u16, parse_u32, parse_u64, parse_u128, parse_usize, parse_f32, parse_f64, parse_bool, from_i8, from_i16, from_i32, from_i64, from_i128, from_isize, from_u8, from_u16, from_u32, from_u64, from_u128, from_usize, from_f32, from_f64, from_bool, from_char, ParseFailure, NumericOverflow};

flow signed_roundtrips() -> bool {
    let a = match parse_i8("-128") { Ok(value) => from_i8(value) == "-128", Err(_) => false };
    let b = match parse_i16("-32768") { Ok(value) => from_i16(value) == "-32768", Err(_) => false };
    let c = match parse_i32("-2147483648") { Ok(value) => from_i32(value) == "-2147483648", Err(_) => false };
    let d = match parse_i64("-9223372036854775808") { Ok(value) => from_i64(value) == "-9223372036854775808", Err(_) => false };
    let e = match parse_i128("-170141183460469231731687303715884105728") { Ok(value) => from_i128(value) == "-170141183460469231731687303715884105728", Err(_) => false };
    let f = match parse_isize("-42") { Ok(value) => from_isize(value) == "-42", Err(_) => false };
    return a && b && c && d && e && f;
}

flow unsigned_roundtrips() -> bool {
    let a = match parse_u8("255") { Ok(value) => from_u8(value) == "255", Err(_) => false };
    let b = match parse_u16("65535") { Ok(value) => from_u16(value) == "65535", Err(_) => false };
    let c = match parse_u32("4294967295") { Ok(value) => from_u32(value) == "4294967295", Err(_) => false };
    let d = match parse_u64("18446744073709551615") { Ok(value) => from_u64(value) == "18446744073709551615", Err(_) => false };
    let e = match parse_u128("340282366920938463463374607431768211455") { Ok(value) => from_u128(value) == "340282366920938463463374607431768211455", Err(_) => false };
    let f = match parse_usize("42") { Ok(value) => from_usize(value) == "42", Err(_) => false };
    return a && b && c && d && e && f;
}

flow float_roundtrips() -> bool {
    let a = match parse_f32("3.4028235e38") {
        Ok(value) => match parse_f32(from_f32(value)) { Ok(roundtrip) => roundtrip == value, Err(_) => false },
        Err(_) => false,
    };
    let b = match parse_f64("1.7976931348623157e308") {
        Ok(value) => match parse_f64(from_f64(value)) { Ok(roundtrip) => roundtrip == value, Err(_) => false },
        Err(_) => false,
    };
    return a && b;
}

flow main() -> bool {
    let syntax_error = match parse_i32(" 42") { Err(ParseFailure) => true, _ => false };
    let overflow = match parse_i32("2147483648") { Err(NumericOverflow) => true, _ => false };
    let negative_unsigned = match parse_u8("-1") { Err(ParseFailure) => true, _ => false };
    let float_overflow = match parse_f64("1e10000") { Err(NumericOverflow) => true, _ => false };
    return signed_roundtrips() && unsigned_roundtrips() && float_roundtrips()
        && syntax_error && overflow && negative_unsigned && float_overflow
        && parse_bool("true") == Ok(true) && parse_bool("TRUE") == Err(ParseFailure)
        && from_bool(false) == "false" && from_char('😀') == "😀";
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn checked_text_surface_matrix_executes_every_non_numeric_family() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.{len, char_count, is_empty, is_ascii, chars, char_indices, lines, contains, starts_with, ends_with, find, rfind, find_from, find_all, count, split, split_iter, split_once, rsplit_once, splitn, split_whitespace, char_get, byte_slice, char_slice, get_byte_slice, get_char_slice, is_char_boundary, trim, trim_start, trim_end, strip_prefix, strip_suffix, lowercase, uppercase, ascii_lowercase, ascii_uppercase, eq_ignore_ascii_case, replace, replace_n, replace_range, concat, join, repeat, pad_start, pad_end, reverse_chars, format, from_char, TextError};

flow main() -> bool ![Error<TextError>] {
    var scalar_text = "";
    for scalar in chars("Aé") limit Iterations(3) {
        scalar_text = scalar_text + from_char(scalar);
    }
    var scalar_offsets = [];
    for (offset, _) in char_indices("Aé") limit Iterations(3) {
        scalar_offsets = scalar_offsets.push(offset);
    }
    var line_values = [];
    for line in lines("a\r\n\nb\rc\n") limit Iterations(5) {
        line_values = line_values.push(line);
    }
    var lazy_split = [];
    for part in split_iter("a,,b,", ",") limit Iterations(5) {
        lazy_split = lazy_split.push(part);
    }
    var whitespace_parts = [];
    for part in split_whitespace(" a b\n c ") limit Iterations(4) {
        whitespace_parts = whitespace_parts.push(part);
    }
    var matches = [];
    for range in find_all("ab", "") limit Iterations(4) {
        matches = matches.push(range);
    }
    return len("é") == 2 && char_count("é") == 1 && is_empty("") && !is_empty("x")
        && is_ascii("abc") && !is_ascii("é") && scalar_text == "Aé"
        && scalar_offsets == [0, 1] && line_values == ["a", "", "b\rc"]
        && contains("abc", "b") && starts_with("abc", "a") && ends_with("abc", "c")
        && find("éaé", "é") == Some(0) && rfind("éaé", "é") == Some(3)
        && find_from("éaé", "é", 3) == Some(3) && count("aaaa", "aa") == 2
        && matches == [(0, 0), (1, 1), (2, 2)]
        && split("ab", "") == ["", "a", "b", ""]
        && lazy_split == ["a", "", "b", ""]
        && split_once("a=b=c", "=") == Some(("a", "b=c"))
        && rsplit_once("a=b=c", "=") == Some(("a=b", "c"))
        && splitn("a,b,c", ",", 0) == [] && splitn("a,b,c", ",", 1) == ["a,b,c"]
        && splitn("a,b,c", ",", 2) == ["a", "b,c"]
        && whitespace_parts == ["a", "b", "c"]
        && char_get("é", 0) == Some('é') && char_get("é", 1) == None
        && byte_slice("é", Range.closed(0, 1)) == "é"
        && char_slice("Aé", Range.closed(1, 1)) == "é"
        && get_byte_slice("é", Range.closed(1, 1)) == None
        && get_char_slice("Aé", Range.closed(1, 1)) == Some("é")
        && is_char_boundary("é", 0) && !is_char_boundary("é", 1) && is_char_boundary("é", 2)
        && trim("  x  ") == "x" && trim_start("  x ") == "x " && trim_end(" x  ") == " x"
        && strip_prefix("prefix", "pre") == Some("fix") && strip_suffix("suffix", "fix") == Some("suf")
        && lowercase("İΣ") == "i̇ς" && uppercase("ß") == "SS"
        && ascii_lowercase("ÄABC") == "Äabc" && ascii_uppercase("äabc") == "äABC"
        && eq_ignore_ascii_case("Ascii-123", "aSCII-123")
        && replace("ab", "", "-") == "-a-b-" && replace_n("aaaa", "a", "b", 2) == "bbaa"
        && replace_range("abcd", Range.closed(1, 2), "X") == "aXd"
        && concat([]) == "" && concat(["a", "é"]) == "aé"
        && join([], ",") == "" && join(["a", "b"], ",") == "a,b"
        && repeat("", 1000000) == "" && repeat("ab", 3) == "ababab"
        && pad_start("é", 3, 'x') == "xxé" && pad_end("é", 3, 'x') == "éxx"
        && reverse_chars("A😀é") == "́e😀A"
        && format("{{{0}}}-{1}-{0}", ["A", "B"]) == Ok("{A}-B-A");
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn checked_text_failure_uses_the_declared_error_effect() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.{byte_slice, TextError, NotCharBoundary};
flow main() -> string {
    return handle {
        return byte_slice("é", Range.closed(1, 1));
    } with {
        Error<TextError>.raise(NotCharBoundary) => { finish "caught"; }
    };
}

"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("caught".into())));
}

#[tokio::test(flavor = "current_thread")]
async fn checked_unicode_text_calls_use_the_pinned_runtime_semantics() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.reverse_chars;
import std.text.{byte_slice, get_byte_slice, is_char_boundary, split_whitespace, trim, TextError};
import std.text.unicode.{normalize, is_normalized, casefold, graphemes, grapheme_indices, grapheme_count, grapheme_slice, reverse_graphemes, NFC, NFD, NFKC, NFKD};
import std.char.{is_whitespace as char_is_whitespace, is_alphabetic, is_alphanumeric, is_control, is_ascii_digit, to_u32, from_u32};
flow main() -> bool ![Error<TextError>] {
    let decomposed = "e" + "́";
    let composed = normalize(decomposed, NFC);
    let roundtrip = normalize(composed, NFD);
    let folded = casefold("Straße");
    var clusters = [];
    for cluster in graphemes("A👩‍🚀é") limit Iterations(4) {
        clusters = clusters.push(cluster);
    }
    var cluster_indices = [];
    for (offset, _) in grapheme_indices("A👩‍🚀é") limit Iterations(4) {
        cluster_indices = cluster_indices.push(offset);
    }
    var words = [];
    for word in split_whitespace("A B\nC") limit Iterations(3) {
        words = words.push(word);
    }
    return composed == "é"
        && roundtrip == decomposed
        && normalize("ﬃ", NFKC) == "ffi" && normalize("é", NFKD) == decomposed
        && is_normalized(composed, NFC)
        && !is_normalized(decomposed, NFC)
        && is_normalized("ffi", NFKC) && is_normalized(decomposed, NFKD)
        && folded == "strasse"
        && reverse_chars("A😀é") == "́e😀A"
        && reverse_graphemes("A👩‍🚀é") == "é👩‍🚀A"
        && grapheme_count("A👩‍🚀é") == 3
        && grapheme_slice("A👩‍🚀é", Range.closed(1, 1)) == "👩‍🚀"
        && byte_slice("é", Range.closed(0, 1)) == "é"
        && get_byte_slice("é", Range.closed(1, 1)) == None
        && is_char_boundary("é", 0) && !is_char_boundary("é", 1) && is_char_boundary("é", 2)
        && trim(" text ") == "text"
        && char_is_whitespace(' ') && is_alphabetic('ª') && is_alphanumeric('Ⅷ')
        && is_control('\n') && is_ascii_digit('7') && to_u32('😀') == 128512
        && from_u32(128512) == Some('😀')
        && words == ["A", "B", "C"] && clusters == ["A", "👩‍🚀", "é"]
        && cluster_indices == [0, 1, 12];
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn checked_global_counts_preserve_suspension_unicode_and_sliced_views() {
    let checked = checked_project(
        r#"
module app.main;
import std.collections.len;
import std.collections.is_empty;
import std.io.println;
import std.runtime.checkpoint;
flow text() -> string {
    println("text")?;
    checkpoint("text");
    return "中😀é";
}
flow identity<T>(value: T) -> T { return value; }
flow main() -> bool {
    let array = [1, 2, 3, 4];
    let alias = array;
    let slice = array[1, 3);
    let empty = array[2, 2);
    let list = [1; 2; 3];
    let count = len(identity(text()));
    return count == 10 && len(array) == 4 && len(list) == 3
        && len(slice) == 2 && !is_empty(slice) && is_empty(empty)
        && len(empty) == 0 && len("") == 0 && is_empty("")
        && !is_empty("😀") && len("😀") == 4 && array == alias;
}
"#,
    );
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.stdout_text(), "text\n");
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for checkpoint in [saved, &decoded] {
        let host = FakeHost::new(services);
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.stdout_text(), "");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checked_map_counts_and_unicode_indexing_execute_without_host_calls() {
    let checked = checked_project(
        r#"
module app.main;
import std.collections.len;
import std.collections.is_empty;
import std.text.char_get;
flow main(values: Map<string, string>) -> bool {
    if is_empty(values) { return len(values) == 0; }
    let text = "aé中🙂";
    return len(values) == 2
        && values["first"] == "payload"
        && char_get(text, 0) == Some('a')
        && char_get(text, 1) == Some('é')
        && char_get(text, 2) == Some('中')
        && char_get(text, 3) == Some('🙂');
}
"#,
    );
    for entries in [
        vec![],
        vec![
            (
                InterpValue::String("first".into()),
                InterpValue::String("payload".into()),
            ),
            (
                InterpValue::String("second".into()),
                InterpValue::String("unchanged".into()),
            ),
        ],
    ] {
        let input = InterpValue::Map(entries.into());
        let alias = input.clone();
        let host = FakeHost::new(HostServiceAvailability::default());
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![input.clone()],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(input, alias);
    }
}
