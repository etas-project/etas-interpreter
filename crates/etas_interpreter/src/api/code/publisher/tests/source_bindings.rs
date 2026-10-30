use super::*;

#[tokio::test(flavor = "current_thread")]
async fn publishing_a_revision_does_not_require_or_authorize_its_unanalyzed_original_body() {
    use etas_frontend::{CheckRequest, CheckScope, code::identity::DefinitionOrigins};
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; agent Worker() -> i32 ![] { return 1; } flow main(args: Array<string>) -> i32 { let definition = ^Worker; return 0; }",
    ));
    input.options.entry_policy = EntryPolicy::Runnable;
    let mut session = FrontendSession::new();
    let project = session.open_project(input);
    let output = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap()
        .output;
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
    let reflection = *checked.types.reflections.keys().next().unwrap();
    let catalog =
        Arc::new(DefinitionCatalog::build(checked.clone(), &origins, Default::default()).unwrap());
    let original = catalog.freeze_agent(reflection).unwrap();
    assert!(
        !checked
            .reachability
            .analysis_items
            .contains(&original.agent())
    );
    let store = etas_frontend::code::definition::NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let definition = store.reflect_agent(reflection).unwrap();
    let candidate = store
        .compile_definition(&definition, &mut FrontendSession::new(), Default::default())
        .unwrap();
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    assert!(
        prepared
            .callable_plan(original.agent(), true)
            .unwrap_err()
            .contains("MissingAnalysis")
    );

    let mut revisions = crate::api::code::PreparedRevisionSet::new(checked.clone());
    revisions.insert(prepared.clone()).unwrap();
    let snapshot = |item| crate::orchestration::ExecutionProgramSnapshot {
        program: crate::orchestration::CheckpointProgramReference::Source,
        root: crate::orchestration::ExecutionRootSnapshot::Item(item),
    };
    assert!(
        revisions
            .execution_snapshot(&snapshot(original.agent()))
            .unwrap_err()
            .contains("MissingAnalysis")
    );
    let main = revisions
        .execution_snapshot(&snapshot(checked.entry.unwrap()))
        .unwrap();
    assert_eq!(main.plan.entry.item, checked.entry.unwrap());
    let result = prepared
        .create_run(
            vec![],
            &crate::testing::FakeHost::new(Default::default()),
            Default::default(),
        )
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&crate::value::InterpValue::i32(1)));
}

async fn run_source_flow(
    prepared: &Arc<PreparedAgentRevision>,
    name: &str,
) -> crate::api::RunOutcome {
    run_source_flow_with_limit(prepared, name, None).await
}

async fn run_source_flow_with_limit(
    prepared: &Arc<PreparedAgentRevision>,
    name: &str,
    max_steps: Option<u64>,
) -> crate::api::RunOutcome {
    use crate::{
        control::CallTarget,
        eval::{EvalContext, EvalContextInput},
    };
    let checked = prepared.program();
    let item = checked
        .hir
        .items
        .iter()
        .find_map(|(item, declaration)| {
            let etas_hir::HirItem::Flow(flow) = declaration else {
                return None;
            };
            (checked.symbols.get(flow.symbol)?.name == name).then_some(item)
        })
        .unwrap();
    let mut options = crate::api::RunOptions::default();
    options.execution_limits.max_steps =
        max_steps.map(|steps| std::num::NonZeroU64::new(steps).unwrap());
    let (program, mut state) = EvalContextInput {
        replay_keys: crate::eval::replay::RunReplayKeys::start(&crate::api::RunOptions::default())
            .unwrap(),
        code_definitions: Some(prepared.definitions.clone()),
        syntax_limits: Default::default(),
        storage_limits: Default::default(),
        event_observer: None,
        execution: etas_host::execution::ExecutionScope::new_owned(),
        checked,
        plan: prepared.plan(),
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps: 0,
        current_session: None,
        entry_item: prepared.entry().item,
        entry_args: &[],
    }
    .into_parts();
    let mut eval = EvalContext::new(program, &mut state);
    eval.revision = Some(prepared);
    let span = checked.hir.items[item].span();
    let signal = eval.execute_call_target_frame(CallTarget::FlowItem(item), vec![], span, None);
    let host = crate::testing::FakeHost::new(Default::default());
    crate::driver::execute_entry(&mut eval, signal, &host)
        .await
        .into_outcome()
}

#[tokio::test(flavor = "current_thread")]
async fn frozen_source_calls_do_not_reset_the_invocations_step_budget() {
    let source = format!(
        "module app; agent Worker() -> i32 {{ return 1; }} flow invoke_original() -> i32 {{ {} return 0; }} flow inspect() -> unit {{ let definition = ^Worker; return; }}",
        "Worker();".repeat(80)
    );
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(&source), "return 2;"))
        .unwrap();
    let outcome = run_source_flow_with_limit(&prepared, "invoke_original", Some(32)).await;
    let crate::api::RunOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault)) = outcome
    else {
        panic!("{outcome:?}")
    };
    assert!(
        fault
            .message
            .contains("maximum interpreter execution steps (32)"),
        "{fault:?}"
    );
}

#[test]
fn native_source_plan_uses_its_selected_entry_instead_of_the_projects_main() {
    let source = "module app; import std.io.println; agent Worker() -> i32 { return 1; } flow main() -> unit { let definition = ^Worker; println(\"not the selected entry\")?; return; }";
    let base = frozen(source);
    let original_entry = base.checked().entry;
    assert!(original_entry.is_some_and(|entry| entry != base.agent()));
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base.clone(), "return 2;"))
        .unwrap();
    assert_eq!(base.checked().entry, original_entry);
    let source_plan = prepared.callable_plan(base.agent(), true).unwrap();
    assert_eq!(
        source_plan.host_requirements.entry,
        Some(etas_effects::InterpreterSupport::LocalOnly)
    );
    assert_ne!(
        base.checked().effects.interpreter_support.entry,
        source_plan.host_requirements.entry
    );
    let invalid =
        crate::plan::build_plan_for_entry(base.checked(), Some(etas_hir::HirItemId(u32::MAX)));
    assert!(invalid.plan.is_none());
    assert!(invalid.diagnostics.iter().any(|diagnostic| diagnostic.code
        == etas_core::DiagnosticCode::Analysis(
            etas_core::AnalysisDiagnosticCode::MissingCheckedFact
        )
        && diagnostic.message.contains("InvalidEntry")));
}

#[tokio::test(flavor = "current_thread")]
async fn frozen_source_calls_suspend_at_console_without_switching_to_the_candidate_body() {
    let source = "module app; import std.io.println; agent Worker() -> i32 { println(\"original\")?; return 1; } flow inspect() -> unit { let definition = ^Worker; return; }";
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(source), "return Worker();"))
        .unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        console: true,
        ..Default::default()
    });
    let mut options = crate::api::RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow("Console", "stdout_write"));
    let result = prepared
        .create_run(vec![], &host, options)
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&crate::value::InterpValue::i32(1)));
    assert_eq!(host.stdout_text(), "original\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn static_agent_calls_and_aliases_use_the_frozen_source_plan() {
    let source = r#"module app;
agent Worker() -> i32 { return 1; }
flow invoke_original() -> i32 { return Worker(); }
flow invoke_alias() -> i32 { let original = Worker; return original(); }
flow inspect() -> unit { let definition = ^Worker; return; }"#;
    let original = frozen(source);
    assert!(original.checked().entry.is_none());
    let prepared = InterpreterCandidatePublisher
        .publish(compile(original.clone(), "return 2;"))
        .unwrap();
    assert!(
        original.checked().entry.is_none(),
        "native entry selection cannot mutate the source snapshot"
    );
    assert_eq!(
        prepared
            .callable_plan(original.agent(), true)
            .unwrap()
            .entry
            .item,
        original.agent()
    );
    for name in ["invoke_original", "invoke_alias"] {
        let outcome = run_source_flow(&prepared, name).await;
        let crate::api::RunOutcome::Completed(value) = outcome else {
            panic!("{name}: {outcome:?}")
        };
        assert_eq!(value, crate::value::InterpValue::i32(1), "{name}");
    }
    let host = crate::testing::FakeHost::new(Default::default());
    let revised = prepared
        .create_run(vec![], &host, Default::default())
        .execute()
        .await
        .unwrap();
    assert_eq!(revised.value(), Some(&crate::value::InterpValue::i32(2)));
}

#[tokio::test(flavor = "current_thread")]
async fn frozen_source_returns_relocate_nominal_identity_to_the_candidate() {
    let source = r#"module app;
type UserId = string;
agent Worker() -> UserId { return UserId("original"); }
flow invoke_original() -> UserId { return Worker(); }
flow inspect() -> unit { let definition = ^Worker; return; }"#;
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(source), "return UserId(\"revision\");"))
        .unwrap();
    let outcome = run_source_flow(&prepared, "invoke_original").await;
    let crate::api::RunOutcome::Completed(crate::value::InterpValue::Nominal { ty, value }) =
        outcome
    else {
        panic!("{outcome:?}")
    };
    let method = prepared
        .targets
        .callables
        .get(&prepared.targets.default)
        .unwrap();
    assert_eq!(ty, method.candidate.output);
    assert_eq!(*value, crate::value::InterpValue::String("original".into()));
}

#[tokio::test(flavor = "current_thread")]
async fn auxiliary_revision_method_has_its_own_intrinsics_and_action_mediation() {
    let base = frozen(
        r#"module app;
import std.io.println;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow loud() -> i32 { println("auxiliary")?; return 7; }
}
flow main(worker: Worker) -> i32 { return worker.loud(); }
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let original = base.checked().clone();
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let method = prepared
        .targets
        .callables
        .iter()
        .find_map(|(id, target)| (id != &prepared.targets.default).then_some(target))
        .unwrap();
    let method_plan = prepared
        .callable_plan(method.candidate_item, false)
        .unwrap();
    assert_eq!(method_plan.entry.item, method.candidate_item);
    assert!(prepared.plan().action_mediation.default_actions.is_empty());
    assert!(!method_plan.action_mediation.default_actions.is_empty());
    assert!(Arc::ptr_eq(&prepared.plan().slots, &method_plan.slots));
    assert!(Arc::ptr_eq(&prepared.plan().frames, &method_plan.frames));
    assert!(Arc::ptr_eq(
        &prepared.plan().arguments,
        &method_plan.arguments
    ));
    assert!(Arc::ptr_eq(
        &prepared.plan().closures,
        &method_plan.closures
    ));
    assert!(Arc::ptr_eq(&prepared.plan().globals, &method_plan.globals));
    assert!(Arc::ptr_eq(
        &prepared.plan().resources,
        &method_plan.resources
    ));
    assert!(std::ptr::eq(
        prepared.plan().dispatch.pure_abi(),
        method_plan.dispatch.pure_abi()
    ));
    let console = etas_std::StdIntrinsicId(etas_std::intrinsic::runtime::IO_PRINTLN);
    let identity = original
        .symbols
        .iter()
        .find_map(|symbol| {
            let identity = method_plan.dispatch.std_intrinsic(symbol.id)?;
            (identity.intrinsic == console).then_some(identity)
        })
        .unwrap();
    assert!(
        method_plan
            .dispatch
            .validate_std_intrinsic(identity)
            .is_ok()
    );
    assert!(
        prepared
            .plan()
            .dispatch
            .validate_std_intrinsic(identity)
            .is_err(),
        "unused auxiliary intrinsic must not become executable in the default entry"
    );
    assert!(
        !crate::host::validate_host_readiness(
            method_plan,
            Default::default(),
            original.hir.items[original.entry.unwrap()].span()
        )
        .is_empty()
    );
    let no_host = crate::testing::FakeHost::new(Default::default());
    let default_result = prepared
        .create_run(vec![], &no_host, Default::default())
        .execute()
        .await
        .unwrap();
    assert!(
        default_result.diagnostics.is_empty(),
        "{:?}",
        default_result.diagnostics
    );
    assert_eq!(
        default_result.value(),
        Some(&crate::value::InterpValue::i32(2))
    );

    let agent = crate::value::AgentRevisionValue::new_in_program(
        prepared.clone(),
        None,
        &original,
        prepared.targets.component,
    )
    .unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        console: true,
        ..Default::default()
    });
    let mut options = crate::api::RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow("Console", "stdout_write"));
    let result = Interpreter
        .run_checked(
            &original,
            EntryPoint {
                item: original.entry.unwrap(),
            },
            vec![crate::value::InterpValue::Callable(
                crate::control::CallTarget::AgentRevision(agent),
            )],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&crate::value::InterpValue::i32(7)));
    assert_eq!(host.stdout_text(), "auxiliary\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn frozen_auxiliary_method_uses_its_own_entry_even_without_an_agent_default() {
    let source = r#"module app;
import std.io.println;
agent Helper;
impl agent Helper { flow loud() -> i32 { println("helper")?; return 7; } }
agent Worker() -> i32 { println("original")?; return 1; }
flow inspect() -> unit { let definition = ^Worker; return; }
"#;
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(source), "return Helper.loud();"))
        .unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        console: true,
        ..Default::default()
    });
    let mut options = crate::api::RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow("Console", "stdout_write"));
    let result = prepared
        .create_run(vec![], &host, options)
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&crate::value::InterpValue::i32(7)));
    assert_eq!(host.stdout_text(), "helper\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn frozen_auxiliary_call_checks_the_selected_methods_host_readiness() {
    let source = r#"module app;
import std.io.println;
agent Worker() -> i32 { return 1; }
impl agent Worker { flow loud() -> i32 { println("must not run")?; return 7; } }
flow invoke_original() -> i32 { return Worker.loud(); }
flow inspect() -> unit { let definition = ^Worker; return; }
"#;
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(source), "return 2;"))
        .unwrap();
    let outcome = run_source_flow(&prepared, "invoke_original").await;
    let crate::api::RunOutcome::Failed(crate::api::RunFailure::Language(diagnostic)) = outcome
    else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        diagnostic.code,
        etas_core::DiagnosticCode::Analysis(etas_core::AnalysisDiagnosticCode::MissingHostHandler)
    );
    assert!(diagnostic.message.contains("Console"), "{diagnostic:?}");
}

#[test]
fn frozen_source_callable_bindings_reject_another_candidate_snapshot() {
    let first = InterpreterCandidatePublisher
        .publish(compile(frozen(SOURCE), "return 2;"))
        .unwrap();
    let second = InterpreterCandidatePublisher
        .publish(compile(frozen(SOURCE), "return 3;"))
        .unwrap();
    let error = crate::value::AgentRevisionValue::frozen_declaration(
        first.clone(),
        second.program(),
        first.targets.candidate_agent,
    )
    .unwrap_err();
    assert!(error.contains("different candidate program"), "{error}");
    let source = crate::value::AgentRevisionValue::frozen_declaration(
        first.clone(),
        first.program(),
        first.targets.candidate_agent,
    )
    .unwrap();
    let revised = crate::value::AgentRevisionValue::new_in_program(
        first.clone(),
        None,
        first.type_bridge().original_program(),
        first.targets.component,
    )
    .unwrap();
    assert_ne!(source, revised);
}

#[tokio::test(flavor = "current_thread")]
async fn native_prepared_entry_retains_the_revision_and_its_frozen_declaration_bindings() {
    let base = frozen(
        "module app; agent Worker() -> string { return \"baseline\"; } flow inspect() -> unit { let definition = ^Worker; return; }",
    );
    let original = base.checked().clone();
    let prepared = InterpreterCandidatePublisher
        .publish(compile(
            base,
            "let definition = ^Worker; return definition.revision.key;",
        ))
        .unwrap();
    let expr = *original.types.reflections.keys().next().unwrap();
    let baseline = prepared.original_definitions().reflect_agent(expr).unwrap();
    let host = crate::testing::FakeHost::new(Default::default());
    let result = prepared
        .create_run(vec![], &host, Default::default())
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&crate::value::InterpValue::String(
            baseline.revision().key().fingerprint().to_string().into()
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_prepared_entry_rejects_replacement_definitions_even_for_the_same_program() {
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(SOURCE), "return 2;"))
        .unwrap();
    let original = prepared.definitions.definitions();
    let other = Arc::new(etas_frontend::code::definition::NativeDefinitionStore::new(
        original.catalog().clone(),
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    ));
    let options = crate::api::RunOptions {
        code_definitions: Some(Arc::new(crate::api::code::PreparedCodeDefinitions::new(
            other,
        ))),
        ..Default::default()
    };
    let host = crate::testing::FakeHost::new(Default::default());
    let result = prepared
        .create_run(vec![], &host, options)
        .execute()
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("prepared revision definitions cannot be replaced")),
        "{:?}",
        result.diagnostics
    );
}
