use super::*;
use crate::{
    api::{RunOptions, codec},
    value::InterpValue,
};

#[tokio::test(flavor = "current_thread")]
async fn ordinary_flow_checkpoint_roundtrips_and_rejects_forged_target_families() {
    let candidate = flow_candidate_for(
        "module app; import std.runtime.checkpoint; flow toggle(value: bool) -> bool ![] { checkpoint(\"base\"); return value; } flow inspect() -> unit { let definition = ^toggle; return; }",
        "checkpoint(\"revised\"); return !value;",
    );
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        checkpoint: true,
        ..Default::default()
    });
    let result = prepared
        .create_run(vec![InterpValue::Bool(false)], &host, RunOptions::default())
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(result.checkpoints.len(), 1);
    let checkpoint = &result.checkpoints[0];
    let identity = checkpoint.code_revision.as_ref().unwrap();
    assert_eq!(identity.target.kind(), etas_code::DefinitionKind::Flow);
    assert_eq!(
        identity.revision.definition.as_declaration(),
        Some(&identity.target)
    );
    identity.validate().unwrap();

    let mut set = crate::api::code::PreparedRevisionSet::new(
        prepared.type_bridge().original_program().clone(),
    );
    set.insert(prepared.clone()).unwrap();
    let set = Arc::new(set);
    let artifact = codec::checkpoint_artifact_json(&[], "toggle", checkpoint).unwrap();
    let bytes = codec::checkpoint_file_to_bytes(artifact, Default::default()).unwrap();
    let artifact = codec::checkpoint_file_from_bytes(&bytes, Default::default()).unwrap();
    let restored = codec::checkpoint_from_json_with_revisions(
        &Default::default(),
        &artifact,
        prepared.program(),
        Some(&set),
    )
    .unwrap();
    for checkpoint in [checkpoint, &restored] {
        let result = prepared
            .create_resume(
                checkpoint,
                &host,
                RunOptions {
                    checkpoint_revisions: Some(set.clone()),
                    ..Default::default()
                },
            )
            .execute()
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert!(result.checkpoints.is_empty());
    }
    for change in 0..4 {
        let mut altered = identity.clone();
        let target = |kind, declaration: &str| {
            etas_code::DefinitionKey::new(
                identity.target.package(),
                identity.target.module().to_vec(),
                vec![declaration.to_owned()],
                kind,
            )
            .unwrap()
        };
        match change {
            0 => altered.target = target(etas_code::DefinitionKind::Agent, "toggle"),
            1 => altered.target = target(etas_code::DefinitionKind::Flow, "different"),
            2 => {
                altered.revision.definition = etas_code::CallableId::declaration(target(
                    etas_code::DefinitionKind::Flow,
                    "different",
                ))
                .unwrap();
                altered.revision_key = etas_code::RevisionKey::from_inputs(&altered.revision);
            }
            _ => {
                altered.snapshot_fingerprint =
                    CodeFingerprint::of_bytes("etas.test.forged-snapshot", b"other")
            }
        }
        if change < 3 {
            assert!(altered.validate().is_err());
        }
        assert!(set.checked_revision(&altered).is_err());
        let mut document = artifact.clone();
        document["checkpoint"]["code_revision"] = serde_json::to_value(altered).unwrap();
        let error = codec::checkpoint_from_json_with_revisions(
            &Default::default(),
            &document,
            prepared.program(),
            Some(&set),
        )
        .unwrap_err();
        assert!(error.message().contains("revision"), "{change}: {error}");
    }
}
