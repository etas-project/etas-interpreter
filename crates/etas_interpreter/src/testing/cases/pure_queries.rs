use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

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
    let split_prefix = "中😀:";
    let separator_prefix = ":";
    let separated = split(split_prefix + ":tail::", separator_prefix + ":");
    let joined = join(separated, "-" + ":");
    checkpoint("segmented-text-transform");
    return parts == ["A", "中😀", "\ré"]
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
import std.text.{len as text_len, contains, starts_with, ends_with, parse_i32};
import std.runtime.checkpoint;
flow main() -> bool {
    var text = "start";
    for _ in Range.closed(1, 1000) limit Iterations(1000) { text = text + "中😀é"; }
    let previous = text;
    text = text + "end";
    let numeric_prefix = "000";
    let parsed = match parse_i32(numeric_prefix + "2147483647") { Ok(value) => value == 2147483647, Err(_) => false };
    let correct = len(text) == 4008 && text_len(text) == 4008 && !is_empty(text)
        && starts_with(text, "start中") && ends_with(text, "éend")
        && contains(text, "éend") && !contains(text, "absent")
        && text[5] == '中' && text[6] == '😀' && text[7] == 'e' && text[8] == '́'
        && parsed;
    checkpoint("composite-queries");
    text = text + "!";
    return correct && len(previous) == 4005 && len(text) == 4009;
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
import std.text.{parse_i32, ParseError};
import std.runtime.checkpoint;
flow main() -> Result<i32, ParseError> {
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
        panic!("checked nominal ParseError")
    };
    let Some(etas_types::Type::Nominal(nominal)) = checked.type_store.get(*ty) else {
        panic!("checked ParseError identity")
    };
    assert_eq!(nominal.name, "std.text.ParseError");
    assert_eq!(
        value.as_ref(),
        &InterpValue::String(
            "not-an-integer"
                .parse::<i32>()
                .unwrap_err()
                .to_string()
                .into()
        )
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
    return count == 4 && len(array) == 4 && len(list) == 3
        && len(slice) == 2 && !is_empty(slice) && is_empty(empty)
        && len(empty) == 0 && len("") == 0 && is_empty("")
        && !is_empty("😀") && len("😀") == 1 && array == alias;
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
flow main(values: Map<string, string>) -> bool {
    if is_empty(values) { return len(values) == 0; }
    let text = "aé中🙂";
    return len(values) == 2
        && values["first"] == "payload"
        && text[0] == 'a'
        && text[1] == 'é'
        && text[2] == '中'
        && text[3] == '🙂';
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
