use super::super::*;
use crate::api::codec::{
    CheckpointFileLimits, checkpoint_artifact_json, checkpoint_file_from_bytes,
    checkpoint_file_to_bytes, checkpoint_from_json,
};

#[tokio::test(flavor = "current_thread")]
async fn shared_json_adt_construction_matching_and_codecs_agree() {
    let checked = checked_project(
        r#"
module app.main;
import std.json.{JsonValue, parse, stringify};

flow classify(value: JsonValue) -> string {
    return match value {
        JsonValue.Null => "null",
        JsonValue.Bool(flag) => if flag { "true" } else { "false" },
        JsonValue.Number(number) => if number == 3.5 { "number" } else { "wrong" },
        JsonValue.String(text) => text,
        JsonValue.Array(items) => classify(items[0]),
        JsonValue.Object(fields) => match fields.get("item") {
            Some(item) => classify(item), None => "missing",
        },
    };
}


flow main() -> bool {
    let object = JsonValue.Object({"item" => JsonValue.Array([JsonValue.Number(3.5)])});
    let encoded = match stringify(object) { Ok(text) => text, Err(_) => abort("encode") };
    let decoded = match parse(encoded) { Ok(value) => value, Err(_) => abort("decode") };
    let reversed = JsonValue.Object({"z" => JsonValue.Bool(true), "a" => JsonValue.Null});
    let ordered = match parse("{\"a\":null,\"z\":true}") { Ok(value) => value, Err(_) => abort("decode ordered") };
    return object == decoded
        && reversed == ordered
        && classify(decoded) == "number"
        && classify(JsonValue.Null) == "null"
        && classify(JsonValue.Bool(true)) == "true"
        && classify(JsonValue.String("text")) == "text";
}
"#,
    );
    let host = FakeHost::new(HostServiceAvailability::default());
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
}

#[tokio::test(flavor = "current_thread")]
async fn json_aliases_and_saved_content_survive_file_checkpoint_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.json.{JsonValue, parse, stringify};
import std.runtime.checkpoint;
flow decode(text: string) -> JsonValue {
    return match parse(text) {
        Ok(value) => value,
        Err(_) => abort("invalid test JSON")
    };
}
flow encode(value: JsonValue) -> string {
    return match stringify(value) {
        Ok(text) => text,
        Err(_) => abort("cannot encode test JSON")
    };
}
flow main() -> (bool, bool, string, string) {
    var document = decode("{\"items\":[true,{\"value\":7}]}");
    let alias = document;
    let nested = Some(document);
    checkpoint("json-aliases");
    document = decode("{\"replacement\":false}");
    return (document != alias, nested == Some(alias), encode(alias), encode(document));
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
    let expected = InterpValue::Tuple(
        vec![
            InterpValue::Bool(true),
            InterpValue::Bool(true),
            InterpValue::String("{\"items\":[true,{\"value\":7.0}]}".into()),
            InterpValue::String("{\"replacement\":false}".into()),
        ]
        .into(),
    );
    assert_eq!(result.value(), Some(&expected));
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let limits = CheckpointFileLimits::default();
    let bytes = checkpoint_file_to_bytes(
        checkpoint_artifact_json(&[], "main", saved).unwrap(),
        limits,
    )
    .unwrap();
    let document = checkpoint_file_from_bytes(&bytes, limits).unwrap();
    let restored = checkpoint_from_json(&document, &checked).unwrap();
    for snapshot in [saved, &restored] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, snapshot, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&expected));
        assert_eq!(
            checkpoint_file_to_bytes(
                checkpoint_artifact_json(&[], "main", snapshot).unwrap(),
                limits
            )
            .unwrap(),
            bytes
        );
    }
}
