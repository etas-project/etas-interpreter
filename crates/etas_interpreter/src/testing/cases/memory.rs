use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_memory_put_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit {
  ProjectMemory.Papers.put("paper-1", "draft");
  return;
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value().cloned(), Some(value::InterpValue::Unit));
    assert_eq!(host.memory_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_handles_memory_conflict_as_typed_error() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> string ![Memory.write] {
  return handle {
    ProjectMemory.Papers.put("paper-1", "draft");
    "written"
  } with {
    Error<MemoryConflict>.raise(conflict) => {
      finish "conflict";
    }
  };
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory_conflict(etas_host::MemoryConflict {
        expected: Some(crate::testing::host::fake_memory_version("v1")),
        actual: Some(crate::testing::host::fake_memory_version("v2")),
        current_value: Some(HostValue::String("existing".to_owned())),
    });

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("conflict".to_owned()))
    );
    assert_eq!(host.memory_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_exposes_memory_conflict_current_value_as_json() {
    let checked = checked_project(
        r#"
module app.main;
import std.option.unwrap;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> bool ![Memory.write] {
  return handle {
    ProjectMemory.Papers.put("paper-1", "draft");
    false
  } with {
    Error<MemoryConflict>.raise(conflict) => {
      let current = unwrap(conflict.current_value);
      finish true;
    }
  };
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory_conflict(etas_host::MemoryConflict {
        expected: Some(crate::testing::host::fake_memory_version("v1")),
        actual: Some(crate::testing::host::fake_memory_version("v2")),
        current_value: Some(HostValue::String("existing".to_owned())),
    });

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::Bool(true))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_memory_conflict_lookup_does_not_use_short_type_name_scan() {
    let mut checked = checked_project(
        r#"
module app.main;
import std.option.unwrap;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> bool ![Memory.write] {
  return handle {
    ProjectMemory.Papers.put("paper-1", "draft");
    false
  } with {
    Error<MemoryConflict>.raise(conflict) => {
      let current = unwrap(conflict.current_value);
      finish true;
    }
  };
}
"#,
    );
    checked.type_store.intern(Type::Nominal(NominalTypeRef {
        name: "MemoryConflict".to_owned(),
        params: Vec::new(),
        representation: None,
    }));

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory_conflict(etas_host::MemoryConflict {
        expected: Some(crate::testing::host::fake_memory_version("v1")),
        actual: Some(crate::testing::host::fake_memory_version("v2")),
        current_value: Some(HostValue::String("existing".to_owned())),
    });

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::Bool(true))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_memory_region_handle_when_initializer_is_not_std_memory_region() {
    let mut checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit {
  let memory = ProjectMemory;
  return;
}
"#,
    );

    let project_memory_symbol = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == "ProjectMemory")
        .map(|symbol| symbol.id)
        .expect("ProjectMemory symbol");
    let main_symbol = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == "main")
        .map(|symbol| symbol.id)
        .expect("main symbol");
    let project_memory_item = checked
        .hir
        .items
        .iter()
        .find_map(|(item_id, item)| match item {
            HirItem::TopLevelLet(decl) if decl.symbol == project_memory_symbol => Some(item_id),
            _ => None,
        })
        .expect("ProjectMemory item");
    let HirItem::TopLevelLet(project_memory_decl) = checked
        .hir
        .items
        .get_mut(project_memory_item)
        .expect("ProjectMemory item")
    else {
        panic!("expected top-level let");
    };
    let initializer_expr = project_memory_decl.value;
    let span = checked.hir.exprs[initializer_expr].span(&checked.hir.blocks);
    let callee_expr = match checked
        .hir
        .exprs
        .get(initializer_expr)
        .expect("ProjectMemory initializer")
    {
        HirExpr::Call { callee, .. } => *callee,
        _ => panic!("expected ProjectMemory initializer call"),
    };
    let mut main_path = unresolved_path_from_segments(&["main"], span);
    main_path.resolution = ResolveResult::Resolved(main_symbol);
    let HirExpr::Path(path) = checked
        .hir
        .exprs
        .get_mut(callee_expr)
        .expect("initializer callee")
    else {
        panic!("expected initializer callee path");
    };
    *path = main_path;

    let project_memory_symbol_data = checked
        .symbols
        .get_mut(project_memory_symbol)
        .expect("ProjectMemory symbol");
    let SymbolDef::TopLevelLet { initializer, .. } = &mut project_memory_symbol_data.def else {
        panic!("expected top-level let symbol def");
    };
    *initializer = initializer_expr;
    checked
        .types
        .resource_handles
        .remove(&project_memory_symbol);

    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(result.value().cloned(), None);
    assert_eq!(host.memory_call_count(), 0);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
            && diagnostic
                .message
                .contains("memory-region resource handle is missing checked resource handle facts")
    }));
    assert_eq!(result.diagnostics.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_partially_resolved_memory_region_initializer() {
    let mut checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit {
  let memory = ProjectMemory;
  return;
}
"#,
    );

    let project_memory_symbol = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == "ProjectMemory")
        .map(|symbol| symbol.id)
        .expect("ProjectMemory symbol");
    let project_memory_item = checked
        .hir
        .items
        .iter()
        .find_map(|(item_id, item)| match item {
            HirItem::TopLevelLet(decl) if decl.symbol == project_memory_symbol => Some(item_id),
            _ => None,
        })
        .expect("ProjectMemory item");
    let HirItem::TopLevelLet(project_memory_decl) = checked
        .hir
        .items
        .get_mut(project_memory_item)
        .expect("ProjectMemory item")
    else {
        panic!("expected top-level let");
    };
    let initializer_expr = project_memory_decl.value;
    let span = checked.hir.exprs[initializer_expr].span(&checked.hir.blocks);
    let callee_expr = match checked
        .hir
        .exprs
        .get(initializer_expr)
        .expect("ProjectMemory initializer")
    {
        HirExpr::Call { callee, .. } => *callee,
        _ => panic!("expected ProjectMemory initializer call"),
    };
    let mut std_memory_region = unresolved_path_from_segments(&["std", "memory", "region"], span);
    std_memory_region.resolution = ResolveResult::PartiallyResolved(PartialResolution {
        resolved_prefix: None,
        resolved_segments: 0,
        remaining: vec!["std".to_owned(), "memory".to_owned(), "region".to_owned()],
        reason: PartialResolutionReason::PackageResolverRequired,
    });
    let HirExpr::Path(path) = checked
        .hir
        .exprs
        .get_mut(callee_expr)
        .expect("initializer callee")
    else {
        panic!("expected initializer callee path");
    };
    *path = std_memory_region;

    let project_memory_symbol_data = checked
        .symbols
        .get_mut(project_memory_symbol)
        .expect("ProjectMemory symbol");
    let SymbolDef::TopLevelLet { initializer, .. } = &mut project_memory_symbol_data.def else {
        panic!("expected top-level let symbol def");
    };
    *initializer = initializer_expr;
    checked
        .types
        .resource_handles
        .remove(&project_memory_symbol);

    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(result.value().cloned(), None);
    assert_eq!(host.memory_call_count(), 0);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
            && diagnostic
                .message
                .contains("memory-region resource handle is missing checked resource handle facts")
    }));
    assert_eq!(result.diagnostics.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_exposes_memory_conflict_versions_to_handler() {
    let checked = checked_project(
        r#"
module app.main;
import std.memory.{version};
import std.option.unwrap;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> string ![Memory.write] {
  return handle {
    ProjectMemory.Papers.put("paper-1", "draft");
    version("written").opaque
  } with {
    Error<MemoryConflict>.raise(conflict) => {
      finish unwrap(conflict.actual).opaque;
    }
  };
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory_conflict(etas_host::MemoryConflict {
        expected: Some(crate::testing::host::fake_memory_version("v1")),
        actual: Some(crate::testing::host::fake_memory_version("v2")),
        current_value: Some(HostValue::String("existing".to_owned())),
    });

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String(
            crate::testing::host::fake_memory_version("v2")
                .as_token()
                .to_owned()
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_policy_deny_before_memory_boundary() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Reviews: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit ![Memory.write] {
  ProjectMemory.Reviews.put("review-1", "draft");
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_policy_decision(PolicyDecision::Deny {
        reason: "memory denied".to_owned(),
    });
    let policy_ref = HostValue::String("memory-policy".to_owned());

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: Vec::new(),
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: boundary_policy_context(policy_ref.clone()),
                    },
                    trace: TraceContext::root(TraceId(78)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(result.value().cloned(), None);
    assert_eq!(host.policy_call_count(), 1);
    assert_eq!(host.memory_call_count(), 0);
    let requests = host.policy_requests();
    assert_eq!(requests[0].policy_ref, policy_ref);
    assert_eq!(requests[0].subject.kind, "memory");
    assert!(
        requests[0]
            .subject
            .attributes
            .iter()
            .any(|(name, value)| name == "operation"
                && value == &HostValue::String("put".to_owned()))
    );
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("memory policy denied request: memory denied")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_stops_memory_boundary_when_policy_approval_is_denied() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Reviews: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit ![Memory.write] {
  ProjectMemory.Reviews.put("review-1", "draft");
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Approval,
    ]));
    host.seed_policy_decision(PolicyDecision::RequireApproval {
        request: ApprovalRequest {
            id: HostRequestId(901),
            reason: "memory write requires approval".to_owned(),
            requested_grants: Vec::new(),
            trace: TraceContext::root(TraceId(80)),
        },
    });
    host.seed_approval_decision(ApprovalDecision::Denied {
        reason: "operator denied".to_owned(),
    });
    let policy_ref = HostValue::String("memory-approval-policy".to_owned());

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: Vec::new(),
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: boundary_policy_context(policy_ref.clone()),
                    },
                    trace: TraceContext::root(TraceId(80)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(result.value().cloned(), None);
    assert_eq!(host.policy_call_count(), 1);
    assert_eq!(host.approval_call_count(), 1);
    assert_eq!(host.memory_call_count(), 0);
    let requests = host.policy_requests();
    assert_eq!(requests[0].policy_ref, policy_ref);
    assert_eq!(requests[0].subject.kind, "memory");
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("memory policy approval was denied")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_handles_versioned_memory_conflict() {
    let checked = checked_project(
        r#"
module app.main;

import std.memory.{version};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> string ![Memory.write] {
  return handle {
    ProjectMemory.Papers.put("paper-1", "existing");
    let stale = version("mv1:1111111111111111111111111111111111111111111111111111111111111111:00000000000000000000000000000000:0000000000000003");
    ProjectMemory.Papers.put_versioned("paper-1", "draft", stale);
    "written"
  } with {
    Error<MemoryConflict>.raise(conflict) => {
      finish "conflict";
    }
  };
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("conflict".to_owned()))
    );
    assert_eq!(host.memory_call_count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_continues_after_versioned_memory_conflict_handler_finish() {
    let checked = checked_project(
        r#"
module app.main;

import std.memory.{version};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> string ![Memory] {
  ProjectMemory.Papers.put("paper-1", "existing");
  let handled = handle {
    let stale = version("mv1:1111111111111111111111111111111111111111111111111111111111111111:00000000000000000000000000000000:0000000000000003");
    ProjectMemory.Papers.put_versioned("paper-1", "draft", stale);
    "written"
  } with {
    Error<MemoryConflict>.raise(conflict) => {
      finish "conflict";
    }
  };
  ProjectMemory.Papers.upsert("paper-2", handled);
  return handled + "-continued";
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("conflict-continued".to_owned()))
    );
    assert_eq!(host.memory_call_count(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_memory_upsert_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit {
  ProjectMemory.Papers.upsert("paper-1", "draft");
  return;
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value().cloned(), Some(value::InterpValue::Unit));
    assert_eq!(host.memory_call_count(), 1);
    assert_eq!(
        host.memory_value(
            "project_memory",
            &["Papers"],
            &HostValue::String("paper-1".to_owned()),
        ),
        Some(HostValue::String("draft".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_memory_insert_conflicts_when_key_exists() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> string ![Memory.write] {
  return handle {
    ProjectMemory.Papers.insert("paper-1", "draft");
    "inserted"
  } with {
    Error<MemoryConflict>.raise(conflict) => {
      finish "conflict";
    }
  };
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("existing".to_owned()),
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("conflict".to_owned()))
    );
    assert_eq!(
        host.memory_value(
            "project_memory",
            &["Papers"],
            &HostValue::String("paper-1".to_owned()),
        ),
        Some(HostValue::String("existing".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_memory_update_conflicts_when_key_is_missing() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> string ![Memory.write] {
  return handle {
    ProjectMemory.Papers.update("paper-1", "draft");
    "updated"
  } with {
    Error<MemoryConflict>.raise(conflict) => {
      finish "conflict";
    }
  };
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("conflict".to_owned()))
    );
    assert_eq!(
        host.memory_value(
            "project_memory",
            &["Papers"],
            &HostValue::String("paper-1".to_owned()),
        ),
        None
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_memory_get_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Option<string> {
  return ProjectMemory.Papers.get("paper-1");
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft".to_owned()),
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::OptionSome(Box::new(
            value::InterpValue::String("draft".to_owned()),
        )))
    );
    assert_eq!(host.memory_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_bytes_index_from_host_value() {
    let checked = checked_project(
        r#"
module app.main;

import std.option.unwrap;

alias ProjectMemorySchema = MemoryRegion<{
  Blobs: Store<string, bytes>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> u8 {
  let blob = unwrap(ProjectMemory.Blobs.get("blob-1"));
  return blob[1];
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Blobs"],
        HostValue::String("blob-1".to_owned()),
        HostValue::Bytes(vec![7, 42, 9]),
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value().cloned(), Some(value::InterpValue::u8(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_decodes_memory_array_value_from_host_list_by_store_type() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Vectors: Store<string, Array<string>>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Option<Array<string>> {
  return ProjectMemory.Vectors.get("v1");
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Vectors"],
        HostValue::String("v1".to_owned()),
        HostValue::List(vec![
            HostValue::String("a".to_owned()),
            HostValue::String("b".to_owned()),
        ]),
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::OptionSome(Box::new(
            value::InterpValue::Array(value::ArrayValue::new(vec![
                value::InterpValue::String("a".to_owned()),
                value::InterpValue::String("b".to_owned()),
            ])),
        )))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_decodes_memory_list_value_from_host_list_by_store_type() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Chains: Store<string, List<string>>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Option<List<string>> {
  return ProjectMemory.Chains.get("c1");
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Chains"],
        HostValue::String("c1".to_owned()),
        HostValue::List(vec![
            HostValue::String("head".to_owned()),
            HostValue::String("tail".to_owned()),
        ]),
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::OptionSome(Box::new(
            value::InterpValue::List(
                vec![
                    value::InterpValue::String("head".to_owned()),
                    value::InterpValue::String("tail".to_owned()),
                ]
                .into(),
            ),
        )))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_memory_contains_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> bool {
  return ProjectMemory.Papers.contains("paper-1");
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft".to_owned()),
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::Bool(true))
    );
    assert_eq!(host.memory_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_memory_keys_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> List<string> {
  return ProjectMemory.Papers.keys();
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft-1".to_owned()),
    );
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-2".to_owned()),
        HostValue::String("draft-2".to_owned()),
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::List(
            vec![
                value::InterpValue::String("paper-1".to_owned()),
                value::InterpValue::String("paper-2".to_owned()),
            ]
            .into()
        ))
    );
    assert_eq!(host.memory_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_memory_delete_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit {
  ProjectMemory.Papers.delete("paper-1");
  return;
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft".to_owned()),
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value().cloned(), Some(value::InterpValue::Unit));
    assert_eq!(host.memory_call_count(), 1);
    assert_eq!(
        host.memory_value(
            "project_memory",
            &["Papers"],
            &HostValue::String("paper-1".to_owned()),
        ),
        None
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_memory_selection_limit_as_pure_support_value() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> MemorySelection<string> {
  let selected = ProjectMemory.Papers.select("paper");
  return selected.limit(Tokens(2));
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let (region_stable_id, path, kind, predicate, limit) = match result.value().cloned() {
        Some(value::InterpValue::MemorySelection {
            region_stable_id,
            path,
            kind,
            predicate,
            limit,
            ..
        }) => (region_stable_id, path, kind, predicate, limit),
        other => panic!("expected memory selection value, got {other:?}"),
    };
    assert_eq!(region_stable_id, "project_memory");
    assert_eq!(path, vec!["Papers".to_owned()]);
    assert_eq!(kind, value::MemorySelectionKind::Select);
    assert_eq!(limit, Some(2));
    assert_eq!(
        predicate.as_deref(),
        Some(&value::InterpValue::String("paper".to_owned()))
    );
    assert_eq!(host.memory_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_encodes_memory_selection_prompt_data_through_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Prompt {
  let selected = ProjectMemory.Papers.scan().limit(Tokens(2));
  return Prompt.new().data(selected);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-2".to_owned()),
        HostValue::String("second".to_owned()),
    );
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("first".to_owned()),
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(value::InterpValue::Prompt(messages)) = result.value().cloned() else {
        panic!("expected prompt value, got {:?}", result.value().cloned());
    };
    assert_eq!(host.memory_call_count(), 1);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, value::PromptRole::Data);
    assert!(messages[0].text.contains(r#""key":"paper-1""#));
    assert!(messages[0].text.contains(r#""value":"first""#));
    assert!(
        messages[0]
            .text
            .contains(crate::testing::host::fake_memory_version("v1").as_token())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_encodes_related_memory_selection_prompt_data_through_vector_search() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

type Paper = {
  title: string,
  embedding: Array<i32>,
};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, Paper>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Prompt {
  let selected = ProjectMemory.Papers.related_to([1, 0]).limit(Tokens(1));
  return Prompt.new().data(selected);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-close".to_owned()),
        HostValue::Record(vec![
            ("title".to_owned(), HostValue::String("close".to_owned())),
            (
                "embedding".to_owned(),
                HostValue::List(vec![HostValue::Int(1), HostValue::Int(0)]),
            ),
        ]),
    );
    host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-far".to_owned()),
        HostValue::Record(vec![
            ("title".to_owned(), HostValue::String("far".to_owned())),
            (
                "embedding".to_owned(),
                HostValue::List(vec![HostValue::Int(0), HostValue::Int(1)]),
            ),
        ]),
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(value::InterpValue::Prompt(messages)) = result.value().cloned() else {
        panic!("expected prompt value, got {:?}", result.value().cloned());
    };
    assert_eq!(host.memory_call_count(), 1);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].text.contains(r#""key":"paper-close""#));
    assert!(messages[0].text.contains(r#""title":"close""#));
    assert!(!messages[0].text.contains(r#""key":"paper-far""#));
}
