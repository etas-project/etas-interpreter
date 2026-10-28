use super::*;
use crate::{
    api::{RunOptions, codec},
    control::CallTarget,
    value::{AgentRevisionValue, InterpValue},
};

#[tokio::test(flavor = "current_thread")]
async fn a_registered_revision_cannot_replace_a_different_nominal_agent_in_a_checkpoint_slot() {
    let source = r#"
module app;
import std.runtime.checkpoint;
agent Worker() -> i32 { return 19; }
agent Other() -> i32 { return 31; }
flow inspect() -> unit { let a = ^Worker; let b = ^Other; return; }
flow main(value: Worker, other: Other) -> i32 {
    let wrapped = { choice = value, run = value };
    checkpoint("typed values");
    let first = (wrapped.choice)();
    let second = wrapped.run;
    return second();
}
"#;
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.publication",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let catalog = DefinitionCatalog::build(checked.clone(), &origins, Default::default()).unwrap();
    let prepare = |name: &str, body: &str| {
        let expression = checked
            .types
            .reflections
            .iter()
            .find_map(|(expression, fact)| match fact.target {
                etas_types::CheckedReflectionTarget::Agent { declaration, .. }
                    if checked.symbols.get(declaration).unwrap().name == name =>
                {
                    Some(*expression)
                }
                _ => None,
            })
            .unwrap();
        InterpreterCandidatePublisher
            .publish(compile(catalog.freeze_agent(expression).unwrap(), body))
            .unwrap()
    };
    let worker = prepare("Worker", "return 19;");
    let other = prepare("Other", "return 31;");
    let value = |revision: &Arc<PreparedAgentRevision>| {
        AgentRevisionValue::new_in_program(
            revision.clone(),
            None,
            &checked,
            revision.targets.component,
        )
        .unwrap()
    };
    let worker_value = value(&worker);
    let other_value = value(&other);
    let other_reference =
        serde_json::to_value(other_value.checkpoint_reference().unwrap()).unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        checkpoint: true,
        ..Default::default()
    });
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![
                InterpValue::Callable(CallTarget::AgentRevision(worker_value)),
                InterpValue::Callable(CallTarget::AgentRevision(other_value)),
            ],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(19)));
    let artifact = codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let mut revisions = crate::api::code::PreparedRevisionSet::new(checked.clone());
    revisions.insert(worker.clone()).unwrap();
    revisions.insert(other).unwrap();
    let revisions = Arc::new(revisions);
    codec::checkpoint_from_json_with_revisions(
        &Default::default(),
        &artifact,
        &checked,
        Some(&revisions),
    )
    .unwrap();
    for only_records in [false, true] {
        let mut forged = artifact.clone();
        let mut replacements = 0;
        replace_worker(
            &mut forged["checkpoint"]["machine"],
            &serde_json::to_value(worker.identity()).unwrap(),
            &other_reference,
            only_records,
            false,
            &mut replacements,
        );
        assert!(replacements > 0);
        let error = codec::checkpoint_from_json_with_revisions(
            &Default::default(),
            &forged,
            &checked,
            Some(&revisions),
        )
        .unwrap_err();
        assert!(
            error
                .message()
                .contains("differs from its checked slot type"),
            "{error}"
        );
    }
    let mut forged = artifact;
    replace_worker(
        &mut forged["checkpoint"]["args"],
        &serde_json::to_value(worker.identity()).unwrap(),
        &other_reference,
        false,
        false,
        &mut 0,
    );
    let error = codec::checkpoint_from_json_with_revisions(
        &Default::default(),
        &forged,
        &checked,
        Some(&revisions),
    )
    .unwrap_err();
    assert!(
        error
            .message()
            .contains("differs from its checked slot type"),
        "{error}"
    );
}

fn replace_worker(
    value: &mut serde_json::Value,
    worker: &serde_json::Value,
    replacement: &serde_json::Value,
    only_records: bool,
    in_record: bool,
    replacements: &mut usize,
) {
    let kind = value.get("kind").and_then(serde_json::Value::as_str);
    let in_record = in_record || kind == Some("record");
    if kind == Some("agent_revision") {
        if (!only_records || in_record) && value["reference"]["revision"]["target"] == *worker {
            value["reference"] = replacement.clone();
            *replacements += 1;
        }
        return;
    }
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                replace_worker(
                    value,
                    worker,
                    replacement,
                    only_records,
                    in_record,
                    replacements,
                );
            }
        }
        serde_json::Value::Object(fields) => {
            for value in fields.values_mut() {
                replace_worker(
                    value,
                    worker,
                    replacement,
                    only_records,
                    in_record,
                    replacements,
                );
            }
        }
        _ => {}
    }
}
