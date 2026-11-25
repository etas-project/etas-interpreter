use etas_frontend::{
    Frontend, ProjectEntry, ProjectSourceLoadOptions, ProjectSourceLoadScope, ProjectSourceLoader,
};
use etas_interpreter::{
    api::{self, EntryPoint, RunOptions, codec},
    host::{HostFuture, HostServiceAvailability, HostServices},
};
struct PureHost {
    filesystem: etas_host::LocalFilesystemClient,
    requests: std::sync::Mutex<Vec<etas_host::FilesystemRequest>>,
}

#[test]
fn source_async_handlers_transfer_real_tasks_and_resume_checkpoints() {
    let root = std::env::temp_dir().join(format!(
        "etas-source-transfers-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let maintained = etas_std::library::source_checkout_manifest()
        .parent()
        .unwrap()
        .join("library/std");
    for module in ["task", "task/scheduler", "transfer_demo"] {
        std::fs::create_dir_all(root.join("library/std").join(module)).unwrap();
    }
    copy_collection_sources(&maintained, &root);
    for relative in [
        "task/mod.es",
        "task/scope.es",
        "task/complete.es",
        "task/transfers.es",
        "task/scheduler/mod.es",
        "task/scheduler/policy.es",
        "task/scheduler/handlers.es",
        "runtime/checkpoint/mod.es",
    ] {
        copy_source(&maintained, &root, relative);
    }
    let task_body = r#"
        let adapter = std.task.scheduler.handlers(scope);
        return handle {
            let first = std.task.start(() => {
                perform Async.yield();
                std.runtime.checkpoint("20");
                return 20;
            });
            let second = std.task.start(() => {
                perform Async.yield();
                std.runtime.checkpoint("22");
                return 22;
            });
            let left = perform Async.await(first);
            let right = perform Async.await(second);
            left + right
        } with adapter;
"#;
    let selection_body = task_body.replace("perform Async.yield();", "");
    std::fs::write(root.join("library/std/task/verify.es"), format!(r#"
module std.task;
import std.task.Async;
type StatefulScheduler = {{ dispatches: usize }};
impl StatefulScheduler ~ std.task.scheduler.Scheduler {{
    public flow select(var self, ready: Array<std.task.scheduler.ReadyTask>) -> usize ![] {{
        self = StatefulScheduler {{ dispatches = self.dispatches + 1 }};
        return ready.length - 1;
    }}
}}
flow verify_fifo() -> i32 {{
    return Scope.enter(std.task.scheduler.DefaultScheduler, (scope: Scheduler) => {{ {selection_body} }}, false);
}}
flow verify_stateful() -> i32 {{
    let supplied = StatefulScheduler {{ dispatches = 0 }};
    let result = Scope.enter(supplied, (scope: Scheduler) => {{ {selection_body} }}, false);
    if supplied.dispatches != 0 {{ std.core.abort("run changed its supplied policy value"); }}
    return result;
}}
flow verify_yielding() -> i32 {{
    return Scope.enter(std.task.scheduler.DefaultScheduler, (scope: Scheduler) => {{ {task_body} }}, false);
}}
flow verify_try_join() -> i32 {{
    let jobs: Array<() -> i32 ![Error<string>, Async]> = [
        () => {{ perform Async.yield(); perform Error<string>.raise("lower"); }},
        () => perform Error<string>.raise("higher"),
        () => {{
            while true {{ perform Async.yield(); }}
            std.runtime.checkpoint("loser-finished");
            return 99;
        }},
    ];
    return match try_join<i32, string, ![Async]>(jobs) {{
        Err(error) => if error == "lower" {{ 42 }} else {{ 0 }},
        Ok(_) => 0,
    }};
}}
"#)).unwrap();
    let mut input = ProjectSourceLoader::default()
        .load(ProjectSourceLoadOptions {
            project_root: root.clone(),
            roots: vec![root.join("library")],
            entry: ProjectEntry {
                module: None,
                flow: "verify_fifo".into(),
            },
            scope: ProjectSourceLoadScope::FullSourceTree,
            source_kind: None,
            import_search_roots: vec![],
            external_module_roots: vec![],
        })
        .unwrap()
        .input;
    input.options.standard_library_mode = etas_frontend::StandardLibraryMode::SourceBootstrap;
    bind_source_origin(&mut input);
    let original_input = input.clone();
    let mut session = etas_frontend::FrontendSession::with_std_registry(std::sync::Arc::new(
        etas_std::compiler_contract_registry(),
    ));
    let project = session.open_project(input);
    let output = session
        .check(project, etas_frontend::CheckRequest::one_shot())
        .unwrap()
        .output;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    for role in [
        etas_std::StdExecutionRole::Requeue,
        etas_std::StdExecutionRole::Park,
    ] {
        let item = checked
            .hir
            .items
            .iter()
            .find_map(|(item, declaration)| match declaration {
                etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                    body: etas_hir::HirFlowBody::Runtime { role: actual, .. },
                    ..
                }) if *actual == role => Some(item),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            checked.effects.suspension[&etas_effects::EffectUnit::Item(item)],
            etas_effects::suspension::SuspensionSummary::SUSPENDING
        );
        assert!(checked.types.checked_method_calls.values().any(|method| {
            etas_types::method::execution_role(&checked.hir, &checked.types, &method.target)
                == Some(role)
        }));
    }
    assert_eq!(checked.concurrency.continuation_transfers.len(), 2);
    for (entry, labels, checkpoint_count) in [
        ("verify_fifo", Some(["20", "22"]), 2),
        ("verify_stateful", Some(["22", "20"]), 2),
        ("verify_yielding", None, 2),
        ("verify_try_join", None, 0),
    ] {
        let checked = if entry == "verify_fifo" {
            checked.clone()
        } else {
            let mut input = original_input.clone();
            input.entry.flow = entry.into();
            let mut session = etas_frontend::FrontendSession::with_std_registry(
                std::sync::Arc::new(etas_std::compiler_contract_registry()),
            );
            let project = session.open_project(input);
            let output = session
                .check(project, etas_frontend::CheckRequest::one_shot())
                .unwrap()
                .output;
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            output.checked.unwrap()
        };
        let host = PureHost {
            filesystem: etas_host::LocalFilesystemClient::new(Default::default()),
            requests: Default::default(),
        };
        let options = RunOptions::default();
        let first = api::run_checked_blocking(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options.clone(),
        )
        .unwrap();
        assert!(
            first.diagnostics.is_empty(),
            "{entry}: {:?}",
            first.diagnostics
        );
        assert_eq!(
            codec::value_json(first.value().unwrap()).unwrap()["value"],
            "42",
            "{entry}"
        );
        assert_eq!(first.checkpoints.len(), checkpoint_count);
        if entry == "verify_try_join" {
            let cancelled = first.events.iter().find_map(|event| match event {
                api::WorkflowEvent::TaskCancellationRequested { scope, task, .. } => {
                    Some((*scope, *task))
                }
                _ => None,
            });
            let (scope, task) = cancelled.expect("try_join must cancel its pending child");
            assert!(first.events.iter().any(|event| matches!(
                event,
                api::WorkflowEvent::TaskCleanupCompleted { scope: actual_scope, task: actual_task }
                    if (*actual_scope, *actual_task) == (scope, task)
            )));
        }
        if let Some(labels) = labels {
            assert_eq!(
                first
                    .checkpoints
                    .iter()
                    .map(|checkpoint| checkpoint.label.as_deref().unwrap())
                    .collect::<Vec<_>>(),
                labels
            );
        }
        let encode = |checkpoint: &etas_interpreter::api::InterpreterCheckpoint| {
            let document = codec::checkpoint_artifact_json(&[], entry, checkpoint).unwrap();
            let mut bytes = Vec::new();
            document.write_to(&mut bytes).unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        };
        let documents = first.checkpoints.iter().map(encode).collect::<Vec<_>>();
        for document in &documents {
            assert_eq!(
                document["checkpoint"]["tasks"]["metadata"]["policies"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            let restored = codec::checkpoint_from_json(document, &checked).unwrap();
            let resumed =
                api::resume_checkpoint_blocking(&checked, &restored, &host, options.clone())
                    .unwrap();
            assert!(
                resumed.diagnostics.is_empty(),
                "{entry}: {:?}",
                resumed.diagnostics
            );
            assert_eq!(
                codec::value_json(resumed.value().unwrap()).unwrap()["value"],
                "42"
            );
            for checkpoint in &resumed.checkpoints {
                let expected = first
                    .checkpoints
                    .iter()
                    .position(|baseline| baseline.label == checkpoint.label)
                    .unwrap();
                assert_eq!(
                    encode(checkpoint)["checkpoint"]["tasks"]["metadata"]["policies"],
                    documents[expected]["checkpoint"]["tasks"]["metadata"]["policies"]
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
fn copy_source(library: &std::path::Path, root: &std::path::Path, relative: &str) {
    let target = root.join("library/std").join(relative);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::copy(library.join(relative), target).unwrap();
}

fn bind_source_origin(input: &mut etas_frontend::ProjectInput) {
    let package = etas_code::PackageKey::from_name_and_version("std", "0.4.0").unwrap();
    input.environment.local_source_package = Some(package);
    input.environment.stable_source_keys = input
        .sources
        .iter()
        .map(|source| {
            let relative = source
                .path
                .as_ref()
                .unwrap()
                .strip_prefix(&input.project_root)
                .unwrap();
            etas_frontend::ProjectStableSourceKeyInput {
                source: source.id,
                key: etas_code::StableSourceKey::new(
                    etas_code::StableSourceOrigin::Package(package),
                    &relative
                        .components()
                        .map(|part| part.as_os_str().to_str().unwrap().to_owned())
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            }
        })
        .collect();
}

fn copy_collection_sources(library: &std::path::Path, root: &std::path::Path) {
    for module in ["collections", "math", "option", "result"] {
        let mut sources = std::fs::read_dir(library.join(module))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        sources.sort();
        for source in sources {
            let relative = std::path::Path::new(module).join(source);
            copy_source(library, root, relative.to_str().unwrap());
        }
    }
    for relative in [
        "core/mod.es",
        "runtime/error/complete.es",
        "io/error.es",
        "runtime/mod.es",
        "runtime/budget/mod.es",
        "runtime/limits/mod.es",
        "runtime/time/mod.es",
    ] {
        copy_source(library, root, relative);
    }
    let prelude = std::fs::read_to_string(library.join("prelude/mod.es")).unwrap();
    let prelude = prelude
        .lines()
        .filter(|line| !line.starts_with("public import std.io.IOError;"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::create_dir_all(root.join("library/std/prelude")).unwrap();
    std::fs::write(
        root.join("library/std/prelude/mod.es"),
        format!("{prelude}\npublic import std.effects.Error;\n"),
    )
    .unwrap();
    let effects = std::fs::read_to_string(library.join("effects/mod.es")).unwrap();
    let error = &effects[effects.find("@std.role(\"std.effects.Error\")").unwrap()
        ..effects.find("@std.role(\"std.effects.Fs\")").unwrap()];
    std::fs::create_dir_all(root.join("library/std/effects")).unwrap();
    std::fs::write(
        root.join("library/std/effects/error.es"),
        format!("module std.effects;\n{error}"),
    )
    .unwrap();
}

impl HostServices for PureHost {
    fn availability(&self) -> HostServiceAvailability {
        HostServiceAvailability {
            checkpoint: true,
            filesystem: true,
            ..Default::default()
        }
    }
    fn compiler<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::compiler::CompilerRequest,
    ) -> HostFuture<'a, Result<etas_host::compiler::CompilerResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host compiler")
    }
    fn model<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::ModelRequest,
    ) -> HostFuture<'a, Result<etas_host::ModelResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host model")
    }
    fn tool<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::ToolRequest,
    ) -> HostFuture<'a, Result<etas_host::ToolResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host tool")
    }
    fn memory<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::MemoryRequest,
    ) -> HostFuture<'a, Result<etas_host::MemoryResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host memory")
    }
    fn memory_write<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::memory::MemoryWriteRequest,
    ) -> HostFuture<'a, Result<etas_host::memory::MemoryWriteResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host memory_write")
    }
    fn session<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::SessionRequest,
    ) -> HostFuture<'a, Result<etas_host::SessionResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host session")
    }
    fn session_write<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::session::SessionWriteRequest,
    ) -> HostFuture<'a, Result<etas_host::session::SessionWriteResponse, etas_host::HostError>>
    {
        unreachable!("pure iterator verification unexpectedly called host session_write")
    }
    fn filesystem<'a>(
        &'a self,
        operation: etas_host::execution::OperationContext,
        request: etas_host::FilesystemRequest,
    ) -> HostFuture<'a, Result<etas_host::FilesystemResponse, etas_host::HostError>> {
        self.requests.lock().unwrap().push(request.clone());
        Box::pin(async move { self.filesystem.execute_scoped(request, &operation).await })
    }
    fn command<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::CommandRequest,
    ) -> HostFuture<'a, Result<etas_host::CommandResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host command")
    }
    fn command_process<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::CommandProcessRequest,
    ) -> HostFuture<'a, Result<etas_host::CommandProcessResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host command_process")
    }
    fn tcp<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::TcpConnectRequest,
    ) -> HostFuture<'a, Result<etas_host::TcpConnectResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host tcp")
    }
    fn stream<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::StreamRequest,
    ) -> HostFuture<'a, Result<etas_host::StreamResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host stream")
    }
    fn tls<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::TlsConnectRequest,
    ) -> HostFuture<'a, Result<etas_host::TlsConnectResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host tls")
    }
    fn secret<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::SecretRequest,
    ) -> HostFuture<'a, Result<etas_host::SecretResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host secret")
    }
    fn browser<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::BrowserProtocolRequest,
    ) -> HostFuture<'a, Result<etas_host::BrowserProtocolResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host browser")
    }
    fn console<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::console::ConsoleRequest,
    ) -> HostFuture<'a, Result<etas_host::console::ConsoleResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host console")
    }
    fn approval<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::ApprovalRequest,
    ) -> HostFuture<'a, Result<etas_host::ApprovalResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host approval")
    }
    fn policy<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        _request: etas_host::PolicyEvaluationRequest,
    ) -> HostFuture<'a, Result<etas_host::PolicyResponse, etas_host::HostError>> {
        unreachable!("pure iterator verification unexpectedly called host policy")
    }
}
#[test]
fn source_iterators_keep_values_and_resume_checked_next_calls() {
    let root = std::env::temp_dir().join(format!(
        "etas-source-iteration-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let maintained = etas_std::library::source_checkout_manifest();
    let library = maintained.parent().unwrap().join("library/std");
    copy_collection_sources(&library, &root);
    for relative in [
        "runtime/error/complete.es",
        "runtime/budget/mod.es",
        "runtime/mod.es",
        "runtime/checkpoint/mod.es",
    ] {
        let target = root.join("library/std").join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(library.join(relative), target).unwrap();
    }
    let data = std::fs::read_to_string(library.join("runtime/value/mod.es")).unwrap();
    let data = &data[..data.find("    public flow tagged(").unwrap()];
    let target = root.join("library/std/runtime/value/mod.es");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, format!("{data}\n}}\n")).unwrap();
    let json = std::fs::read_to_string(library.join("json/complete.es")).unwrap();
    let json = &json[..json.find("public flow parse(").unwrap()];
    std::fs::create_dir_all(root.join("library/std/json")).unwrap();
    std::fs::write(root.join("library/std/json/mod.es"), json).unwrap();
    for relative in ["memory/context.es", "stream/clock.es"] {
        let target = root.join("library/std").join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(library.join(relative), target).unwrap();
    }
    let memory = std::fs::read_to_string(library.join("memory/complete.es")).unwrap();
    let error = memory
        .lines()
        .find(|line| line.starts_with("public type StorageError ="))
        .unwrap();
    let preparation = std::fs::read_to_string(library.join("memory/preparation.es")).unwrap();
    let methods = ["impl StorageContext {", "impl u128 {"].map(|start| {
        let part = &preparation[preparation.find(start).unwrap()..];
        &part[..part.find("\n}\n").unwrap() + 3]
    });
    std::fs::write(
        root.join("library/std/memory/verification.es"),
        format!(
            "module std.memory;\n{error}\n{}\n{}\n{}",
            methods[0],
            methods[1],
            r#"
public flow fixture_operation_key() -> string ![Error<StorageError>] {
    return StorageContext.current().operation_key();
}
public flow fixture_context_matches() -> bool ![] {
    let context = StorageContext.current();
    return context.max_value_bytes == 524288 && context.max_result_bytes == 2097152
        && context.max_nodes == 12000 && context.max_depth == 40
        && context.max_scan_rows == 2222 && context.max_page_entries == 31
        && context.receipt_retention_seconds == 7200 && context.now_seconds > 1700000000;
}
"#
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("library/std/stream/verification.es"),
        "module std.stream; public flow fixture_millis() -> u128 ![] { return StreamClock.now(); }",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("library/std/text")).unwrap();
    std::fs::copy(
        library.join("text/storage.es"),
        root.join("library/std/text/storage.es"),
    )
    .unwrap();
    let positions = std::fs::read_to_string(library.join("text/positions.es")).unwrap();
    let get = &positions[positions.find("public flow char_get<").unwrap()..];
    let get = &get[..get.find("\n}\n").unwrap() + 3];
    std::fs::write(
        root.join("library/std/text/positions.es"),
        format!("module std.text;\nimport std.core.Index;\n{get}"),
    )
    .unwrap();
    std::fs::write(
        root.join("library/std/collections/verification.es"),
        format!(
            "module std.collections;\n{}",
            r###"public flow numbers(start: i32, end: i32) -> Iter<i32> ![] {
    return Iter { next = () => {
        if start >= end { return None; }
        if start == 3 { std.runtime.checkpoint(start); }
        return Some((start, numbers(start + 1, end)));
    } };
}

public flow sum() -> i32 ![] {
    var total: i32 = 0;
    for value in numbers(1, 5) {
        total = total + value;
        if value == 2 { std.runtime.checkpoint(value); }
    }
    return total;
}

"###
        ),
    )
    .unwrap();
    let fs_source = std::fs::read_to_string(library.join("fs/mod.es")).unwrap();
    let fs_header = &fs_source[..fs_source.find("public flow path<").unwrap()];
    let fs_flows = [
        "public flow root<",
        "public flow relative_name<",
        "public flow read_bytes<",
        "public flow read_bytes_limited<",
        "public flow write_bytes<",
        "public flow list<",
        "public flow list_limited<",
        "public flow stat<",
        "public flow atomic_replace<",
        "public flow create_dir<",
        "public flow rename<",
        "public flow remove_file<",
        "public flow remove_dir<",
    ]
    .map(|start| {
        let source = &fs_source[fs_source.find(start).unwrap()..];
        &source[..source.find("\n}\n").unwrap() + 3]
    });
    std::fs::create_dir_all(root.join("library/std/fs")).unwrap();
    std::fs::write(
        root.join("library/std/fs/mod.es"),
        format!(
            "{fs_header}\n{}\n{}",
            fs_flows.join("\n"),
            r#"
impl WorkspacePath<R> {
    public flow fixture_path(self, relative: string) -> WorkspacePath<R> ![] {
        return WorkspacePath { relative = relative };
    }
    public flow fixture_file(self) -> WorkspacePath<R> ![] {
        return WorkspacePath { relative = "data.bin" };
    }
    public flow fixture_missing(self) -> WorkspacePath<R> ![] {
        return WorkspacePath { relative = "missing.bin" };
    }
}
"#
        ),
    )
    .unwrap();
    let effects = std::fs::read_to_string(library.join("effects/mod.es")).unwrap();
    let file_io = &effects[effects.find("@std.role(\"std.effects.FileIO\")").unwrap()..];
    let file_io = &file_io[..file_io.find(';').unwrap() + 1];
    let fs = &effects[effects.find("@std.role(\"std.effects.Fs\")").unwrap()..];
    std::fs::create_dir_all(root.join("library/std/effects")).unwrap();
    std::fs::write(
        root.join("library/std/effects/mod.es"),
        format!(
            "module std.effects;\nimport std.fs.{{Region, WorkspacePath, FsStat}};\n{file_io}\n{fs}"
        ),
    )
    .unwrap();
    std::fs::write(root.join("library/std/verify.es"),r###"module std.verify;
import std.collections.{numbers, sum};

type Counter = { value: i32 };
impl Counter {
    flow add(var self, amount: i32) -> i32 ![] {
        self = Counter { value = self.value + amount };
        return self.value;
    }
    flow next(var self) -> i32 ![] {
        let previous = self.value;
        self = Counter { value = previous + 1 };
        return previous;
    }
    flow remember(var self) -> Result<i32, i32> ![] {
        self = Counter { value = self.value + 1 };
        return Err(self.value);
    }
    flow fail(var self) -> i32 ![Error<string>] {
        self.add(100);
        return perform Error<string>.raise("failure");
    }
    flow nested(var self) -> unit ![] {
        self.add(1);
        self.add(2);
    }
}
type Cell<T> = { value: T };
impl Cell<T> {
    flow replace(var self, value: T) -> T ![] {
        let previous = self.value;
        self = Cell { value };
        return previous;
    }
}
flow checked_argument(counter: Counter) -> i32 ![] {
    std.runtime.checkpoint(counter.value);
    return counter.value;
}
let numeric_constant: f32 = f32 { bits = 0x7fc01234 };
flow magnitude_overflow() -> i8 ![] {
    let value: i8 = -128;
    return std.math.abs(value);
}
flow number_sum<T ~ std.math.Number>(left: T, right: T) -> T ![] { return left + right; }
flow math_score() -> i32 ![] {
    let small: i8 = -7;
    let wide: u128 = 1267650600228229401496703205377;
    let next: u128 = 1267650600228229401496703205378;
    let negative_zero = f32 { bits = 0x80000000 };
    let zero = f32 { bits = 0 };
    let nan = f32 { bits = 0xffc01234 };
    let other_nan = f32 { bits = 0x7fc05678 };
    let wide_nan = f64 { bits = 0xfff8000000001234 };
    let one = f32 { bits = 0x3f800000 };
    std.core.assert(std.math.abs(negative_zero).bits == zero.bits
        && std.math.abs(zero).bits == zero.bits
        && std.math.abs(f32 { bits = 0xff800000 }).bits == 0x7f800000
        && std.math.abs(f64 { bits = 0xfff0000000000001 }).bits == 0x7ff0000000000001);
    var score: i32 = 0;
    if std.math.abs(small) == 7 { score = score + 1; }
    if std.math.abs(wide).bits == wide.bits { score = score + 1; }
    if std.math.min(wide, next).bits == wide.bits && std.math.max(wide, next).bits == next.bits { score = score + 1; }
    if std.math.abs(nan).bits == 0x7fc01234 { score = score + 1; }
    if std.math.abs(wide_nan).bits == 0x7ff8000000001234 { score = score + 1; }
    if std.math.min(negative_zero, zero).bits == negative_zero.bits && std.math.min(zero, negative_zero).bits == negative_zero.bits { score = score + 1; }
    if std.math.max(negative_zero, zero).bits == zero.bits && std.math.max(zero, negative_zero).bits == zero.bits { score = score + 1; }
    if std.math.min(nan, one).bits == one.bits && std.math.max(one, nan).bits == one.bits { score = score + 1; }
    if std.math.min(nan, other_nan).bits == nan.bits && std.math.max(nan, other_nan).bits == nan.bits { score = score + 1; }
    let left: i32 = 3;
    let right: i32 = 4;
    if number_sum(left, right) == 7 { score = score + 1; }
    if numeric_constant.bits == std.math.abs(nan).bits { score = score + 1; }
    return score;
}
type Documents;
impl Documents ~ std.fs.Region;
flow fs_lifecycle() -> unit {
    let root = std.fs.root<Documents>();
    let parent = root.fixture_path("lifecycle");
    let directory = root.fixture_path("lifecycle/nested");
    let original = root.fixture_path("lifecycle/nested/original.bin");
    let renamed = root.fixture_path("lifecycle/nested/renamed.bin");
    var intercepted = false;
    handle {
        std.fs.create_dir(root.fixture_path("handler-only"));
    } with {
        std.effects.Fs.create_dir<Documents>(_, _) => {
            intercepted = true;
            resume ();
        }
    };
    std.core.assert(intercepted);
    std.fs.create_dir(directory, true);
    std.fs.create_dir(directory);
    std.fs.create_dir(root);
    let metadata = std.fs.stat(directory);
    let root_metadata = std.fs.stat(root);
    std.core.assert(metadata.is_dir && !metadata.is_file && metadata.len == 0);
    std.core.assert(root_metadata.is_dir && root_metadata.len == 0);
    std.fs.write_bytes(original, "original".utf8);
    std.fs.write_bytes(renamed, "kept".utf8);
    let collision: bool = handle {
        std.fs.rename(original, renamed);
        false
    } with {
        Error<std.io.IOError>.raise(std.io.IOError.AlreadyExists) => { finish true; }
    };
    std.core.assert(collision);
    std.core.assert(std.fs.read_bytes(original) == "original".utf8);
    std.core.assert(std.fs.read_bytes(renamed) == "kept".utf8);
    std.fs.remove_file(renamed);
    std.fs.rename(original, renamed);
    std.core.assert(std.fs.read_bytes(renamed) == "original".utf8);
    std.fs.remove_file(renamed);
    std.fs.remove_dir(directory);
    std.fs.remove_dir(parent);
}
flow fs_score() -> i32 {
    let root = std.fs.root<Documents>();
    let path = root.fixture_file();
    var score: i32 = if std.fs.relative_name(root) == "." { 1 } else { 0 };
    match perform std.effects.Fs.read<Documents>(path, None) {
        Ok(bytes) => { if bytes.length == 4 { score = score + 1; } },
        Err(_) => {},
    };
    let bytes = std.fs.read_bytes_limited(path, 4);
    if bytes.length == 4 { score = score + 1; }
    let exceeded: bool = handle {
        let body = std.fs.read_bytes_limited(path, 2);
        false
    } with {
        Error<std.runtime.error.IOError>.raise(std.io.IOError.LimitExceeded) => { finish true; }
    };
    if exceeded { score = score + 1; }
    let original = std.fs.read_bytes(path);
    if original.length == 4 { score = score + 1; }
    let metadata = std.fs.stat(path);
    if metadata.is_file && !metadata.is_dir && metadata.len == 4 { score = score + 1; }
    std.runtime.checkpoint((path, metadata));
    std.fs.write_bytes(path, "abc".utf8);
    if std.fs.read_bytes(path).length == 3 { score = score + 1; }
    std.fs.atomic_replace(path, original);
    if std.fs.read_bytes(path).length == 4 { score = score + 1; }
    match perform std.effects.Fs.list<Documents>(root, None) {
        Ok(entries) => {
            for entry in entries {
                if std.fs.relative_name(entry) == "data.bin" { score = score + 1; }
            }
        },
        Err(_) => {},
    };
    match perform std.effects.Fs.list<Documents>(root, Some((2, 17))) {
        Ok(entries) => {
            for entry in entries {
                if std.fs.relative_name(entry) == "data.bin" { score = score + 1; }
            }
        },
        Err(_) => {},
    };
    match perform std.effects.Fs.list<Documents>(root, Some((0, 8))) {
        Err(std.fs.ListLimitError.LimitExceeded(max_entries, max_bytes)) => {
            if max_entries == 0 && max_bytes == 8 { score = score + 1; }
        },
        Ok(_) => {},
    };
    var names = "";
    for entry in std.fs.list(root) {
        if names != "" { names = names + ","; }
        names = names + std.fs.relative_name(entry);
    }
    if names == "alpha.bin,data.bin" { score = score + 1; }
    var limited_names = "";
    for entry in std.fs.list_limited(root, 2, 17) {
        if limited_names != "" { limited_names = limited_names + ","; }
        limited_names = limited_names + std.fs.relative_name(entry);
    }
    if limited_names == "alpha.bin,data.bin" { score = score + 1; }
    let list_limit: bool = handle {
        let entries = std.fs.list_limited(root, 1, 17);
        false
    } with {
        Error<std.io.IOError>.raise(std.fs.IOError.LimitExceeded) => { finish true; }
    };
    if list_limit { score = score + 1; }
    var ordered: std.collections.OrderedMap<string, i32> = std.collections.OrderedMap.new();
    ordered = ordered.insert("c", 3).insert("a", 1).insert("b", 2);
    if ordered.get("b") == Some(2) && ordered.contains_key("a") && ordered.len() == 3 {
        score = score + 1;
    }
    match std.fs.read_bytes(root.fixture_missing())? {
        Err(std.runtime.error.IOError.NotFound) => { score = score + 1; },
        _ => std.core.abort("filesystem error lost its shared identity"),
    };
    std.core.assert(score == 16);
    return score;
}
type CapturedRecord = { answer: i32, label: string };
flow capture_score() -> i32 {
    var score = 0;
    let tuple = std.runtime.value.Data.capture((true, 42, "ok", Some(7), std.json.JsonValue.Array([std.json.JsonValue.Number(0.5), std.json.JsonValue.Null])));
    let expected = std.runtime.value.Data.List([
        std.runtime.value.Data.Bool(true), std.runtime.value.Data.Int(42),
        std.runtime.value.Data.String("ok"),
        std.runtime.value.Data.Variant("Some", [std.runtime.value.Data.Int(7)]),
        std.runtime.value.Data.Json(std.json.JsonValue.Array([std.json.JsonValue.Number(0.5), std.json.JsonValue.Null])),
    ]);
    if tuple == Ok(expected) { score = score + 1; }
    match std.runtime.value.Data.capture(CapturedRecord { answer = 42, label = "ok" }) {
        Ok(std.runtime.value.Data.Record(fields)) => { if fields == [("answer", std.runtime.value.Data.Int(42)), ("label", std.runtime.value.Data.String("ok"))] { score = score + 1; } },
        _ => {},
    };
    let neutral = std.runtime.value.Data.Map([(std.runtime.value.Data.String("key"), expected)]);
    if std.runtime.value.Data.capture(neutral) == Ok(neutral) { score = score + 1; }
    let locator = std.runtime.value.Data.List([
        std.runtime.value.Data.Record([("relative", std.runtime.value.Data.String("."))]),
    ]);
    if std.runtime.value.Data.capture([std.fs.root<Documents>()]) == Ok(locator) { score = score + 1; }
    match std.runtime.value.Data.capture((42, () => 1)) {
        Err(error) => { if error.code == "schema_mismatch" { score = score + 1; } },
        _ => {},
    };
    var large = "xx";
    var i: usize = 0;
    while i < 20 { large = large + large; i = i + 1; }
    match std.runtime.value.Data.capture(large) {
        Err(error) => { if error.code == "budget_exceeded" { score = score + 1; } },
        _ => {},
    };
    large = "";
    std.runtime.checkpoint(neutral);
    return score;
}
flow observation_score() -> i32 {
    let context_matches = std.memory.fixture_context_matches();
    let milliseconds = std.stream.fixture_millis();
    std.runtime.checkpoint(milliseconds);
    return (if context_matches { 1 } else { 0 }) + (if std.stream.fixture_millis() >= milliseconds { 1 } else { 0 });
}
flow verify(overflow: bool, crash: bool) -> i32 {
    fs_lifecycle();
    if crash { std.core.assert(false); }
    if overflow { return i32 { bits = magnitude_overflow().bits }; }
    let checkpoint: (i32) -> unit ![] = std.runtime.checkpoint;
    checkpoint(42);
    var counter = Counter { value = 1 };
    let snapshot = counter;
    let first_update = counter.add(counter.next());
    counter.nested();
    var cell = Cell { value = 7 };
    let previous = cell.replace(9);
    counter.add(checked_argument(counter));
    let failure = match counter.remember() { Err(value) => value, Ok(_) => 0 };
    let escaped = counter.fail()?;
    let rejected = match escaped { Err(error) => if error == "failure" { 1 } else { 0 }, Ok(_) => 0 };
    std.runtime.checkpoint(counter);
    let updates = first_update + snapshot.value + counter.value + previous + cell.value + failure + rejected;
    let left: usize = 1;
    let right: usize = 3;
    let range = std.collections.Range.closed(left, right);
    std.runtime.checkpoint(range);
    var range_sum: usize = 0;
    for number in range { range_sum = range_sum + number; }
    let open = std.collections.Range.open(left, right);
    for number in open { range_sum = range_sum + number; }
    let literal = [left, right);
    let endpoints = if range.include_start && range.include_end && !open.include_start && !open.include_end {
        range.start + range.end + literal.start + literal.end
    } else { 0 };
    var digits: i32 = 0;
    var position: i32 = 0;
    while position < 4 {
        position = position + 1;
        let digit: i32 = if position == 2 { continue; } else if position == 4 { break; } else { position };
        digits = digits + digit;
    }
    let values: Array<i32> = [7, 8];
    let first_index: usize = 0;
    let missing_index: usize = 2;
    let indexed = values[first_index]?;
    let missed = values[missing_index]?;
    std.runtime.checkpoint(missed);
    let budget = std.runtime.budget.usd(1.25);
    std.runtime.checkpoint(budget);
    let indexing = match (indexed, missed) {
        (Ok(value), Err(_)) => value + 1,
        _ => 0,
    };
    let original = numbers(1, 5);
    var first: i32 = 0;
    for value in original { first = first + value; if value == 2 { std.runtime.checkpoint(value); } }
    var second: i32 = 0;
    for value in original {
        if value == 2 { continue; }
        second = second + value;
        if value == 3 { break; }
    }
    var mapped: u128 = 0;
    for value in original.map((value: i32) => value.bits).map((value: u128) => value.bits) {
        mapped = mapped + value;
    }
    let bytes_value = bytes { data = [2, 3] };
    var byte_sum: u128 = 0;
    for value in bytes_value { byte_sum = byte_sum + value.bits; if value.bits == 2 { std.runtime.checkpoint(value); } }
    let math = math_score();
    std.core.assert(math == 11);
    return first * 100 + second * 10 + sum() + i32 { bits = byte_sum + mapped + range_sum.bits + endpoints.bits } + indexing + digits + updates + math + fs_score() + capture_score() + observation_score();
}
"###).unwrap();

    let mut loaded = ProjectSourceLoader::default()
        .load(ProjectSourceLoadOptions {
            project_root: root.clone(),
            roots: vec![root.join("library")],
            entry: ProjectEntry {
                module: None,
                flow: "verify".into(),
            },
            scope: ProjectSourceLoadScope::FullSourceTree,
            source_kind: None,
            import_search_roots: vec![],
            external_module_roots: vec![],
        })
        .unwrap();
    bind_source_origin(&mut loaded.input);
    let output = Frontend.check_standard_library_source(loaded.input.clone());
    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let primitives = checked
        .hir
        .items
        .iter()
        .filter_map(|(item, declaration)| match declaration {
            etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                body: etas_hir::HirFlowBody::Runtime { role, .. },
                ..
            }) => Some((item, *role)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(primitives.len(), 8);
    for (item, role) in &primitives {
        if matches!(
            role,
            etas_std::StdExecutionRole::StorageContext
                | etas_std::StdExecutionRole::StorageNonce
                | etas_std::StdExecutionRole::StreamClock
        ) {
            let summary = &checked.effects.item_effects[item];
            assert_eq!(
                summary.determinism,
                etas_effects::Determinism::NonDeterministic
            );
            assert_eq!(
                summary.independent_determinism,
                Some(etas_effects::Determinism::NonDeterministic)
            );
            assert!(
                checked.effects.suspension[&etas_effects::EffectUnit::Item(*item)]
                    .proven_non_suspending()
            );
        }
    }
    for name in [
        "fixture_millis",
        "fixture_context_matches",
        "fixture_operation_key",
    ] {
        let item = checked
            .hir
            .items
            .iter()
            .find_map(|(item, declaration)| match declaration {
                etas_hir::HirItem::Flow(flow)
                    if checked.hir.symbols.get(flow.symbol).unwrap().name == name =>
                {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            checked.effects.item_effects[&item].determinism,
            etas_effects::Determinism::NonDeterministic
        );
        assert_eq!(
            checked.effects.item_effects[&item].independent_determinism,
            Some(etas_effects::Determinism::NonDeterministic)
        );
    }
    let checkpoint_item = primitives
        .iter()
        .find(|(_, role)| *role == etas_std::StdExecutionRole::Checkpoint)
        .unwrap()
        .0;
    assert!(
        checked
            .effects
            .native_call_targets
            .values()
            .any(|targets| targets.contains(&etas_effects::EffectUnit::Item(checkpoint_item)))
    );
    let item = checked
        .hir
        .items
        .iter()
        .find_map(|(item, data)| match data {
            etas_hir::HirItem::Flow(flow)
                if checked.hir.symbols.get(flow.symbol).unwrap().name == "verify" =>
            {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    let workspace = etas_host::TestWorkspace::create("source-filesystem-boundary").unwrap();
    std::fs::write(workspace.path().join("data.bin"), [1u8, 2, 3, 4]).unwrap();
    std::fs::write(workspace.path().join("alpha.bin"), [5u8]).unwrap();
    let region = etas_host::WorkspaceRegionId::new("std.verify.Documents").unwrap();
    let workspace_root = workspace.root().unwrap();
    let mut regions = etas_host::WorkspaceRegionRegistry::default();
    regions
        .insert(region.clone(), workspace_root.clone())
        .unwrap();
    let host = PureHost {
        filesystem: etas_host::LocalFilesystemClient::new(regions),
        requests: Default::default(),
    };
    let mut options = RunOptions::default();
    for action in [
        "read",
        "write",
        "list",
        "stat",
        "atomic_replace",
        "create_dir",
        "rename",
        "remove_file",
        "remove_dir",
    ] {
        options
            .host_context
            .authority
            .grants
            .push(etas_host::HostActionGrant::allow_with_args(
                "Fs",
                action,
                vec![etas_host::ActionArgPattern::Exact(
                    etas_host::HostValue::String(region.as_str().into()),
                )],
            ));
    }
    options.host_context.authority.sandbox = etas_host::SandboxPolicy::allow_listed(
        etas_host::FilesystemPolicy::allow_destructive_workspace(workspace_root),
        etas_host::NetworkPolicy::deny_all(),
        etas_host::CommandPolicy::deny_all(),
        etas_host::DestructiveOpPolicy::allow_workspace_delete(),
    );
    options.host_context.trace = etas_host::TraceContext::root(etas_host::TraceId(91));

    options.storage_limits.max_value_bytes = 524288;
    options.storage_limits.max_result_bytes = 2097152;
    options.storage_limits.max_nodes = 12000;
    options.storage_limits.max_depth = 40;
    options.storage_limits.max_scan_rows = 2222;
    options.storage_limits.max_page_entries = 31;
    options.storage_limits.max_receipt_retention_seconds = 7200;

    let first = api::run_checked_blocking(
        &checked,
        EntryPoint { item },
        vec![api::InterpValue::Bool(false), api::InterpValue::Bool(false)],
        &host,
        options.clone(),
    )
    .unwrap();
    let value = first.value().map(|value| {
        let document = codec::value_json(value).unwrap();
        (*document).clone()
    });
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(value.as_ref().unwrap()["value"], "1175");
    assert_eq!(first.checkpoints.len(), 16);
    {
        let requests = host.requests.lock().unwrap();
        assert_eq!(requests.len(), 32);
        for request in requests.iter() {
            assert_eq!(request.authority, options.host_context.authority);
            assert_eq!(request.trace, options.host_context.trace);
            assert_eq!(
                request.budget.limits(),
                options.host_context.budget.limits()
            );
            let path = match &request.operation {
                etas_host::FilesystemOperation::Read { path }
                | etas_host::FilesystemOperation::ReadLimited { path, .. }
                | etas_host::FilesystemOperation::Write { path, .. }
                | etas_host::FilesystemOperation::AtomicReplace { path, .. }
                | etas_host::FilesystemOperation::CreateDir { path, .. }
                | etas_host::FilesystemOperation::RemoveFile { path }
                | etas_host::FilesystemOperation::RemoveDir { path }
                | etas_host::FilesystemOperation::Rename { source: path, .. }
                | etas_host::FilesystemOperation::ReadDir { path }
                | etas_host::FilesystemOperation::ReadDirLimited { path, .. }
                | etas_host::FilesystemOperation::Stat { path } => path,
                _ => panic!("unexpected source filesystem operation"),
            };
            assert_eq!(path.region, region);
            if let etas_host::FilesystemOperation::Rename { destination, .. } = &request.operation {
                assert_eq!(destination.region, region);
                assert_eq!(
                    destination.relative.to_str(),
                    Some("lifecycle/nested/renamed.bin")
                );
            }
        }
    }
    assert!(!workspace.path().join("lifecycle").exists());
    assert!(!workspace.path().join("handler-only").exists());

    let mut resumed = 0;
    for checkpoint in &first.checkpoints {
        let document = codec::checkpoint_artifact_json(&[], "verify", checkpoint).unwrap();
        let mut bytes = Vec::new();
        document.write_to(&mut bytes).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let restored = codec::checkpoint_from_json(&json, &checked).unwrap();
        let result =
            api::resume_checkpoint_blocking(&checked, &restored, &host, options.clone()).unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.value().map(|value| {
                let document = codec::value_json(value).unwrap();
                (*document).clone()
            }),
            value
        );
        resumed += 1;
    }
    assert_eq!(resumed, 16);
    let mut key_input = loaded.input;
    key_input.entry.flow = "fixture_operation_key".into();
    let key_output = Frontend.check_standard_library_source(key_input);
    assert!(
        key_output.diagnostics.is_empty(),
        "{:?}",
        key_output.diagnostics
    );
    let key_checked = key_output.checked.unwrap();
    let key_item = key_checked
        .hir
        .items
        .iter()
        .find_map(|(item, declaration)| match declaration {
            etas_hir::HirItem::Flow(flow)
                if key_checked.hir.symbols.get(flow.symbol).unwrap().name
                    == "fixture_operation_key" =>
            {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    let before_key = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let key_run = api::run_checked_blocking(
        &key_checked,
        EntryPoint { item: key_item },
        vec![],
        &host,
        options.clone(),
    )
    .unwrap();
    assert!(key_run.diagnostics.is_empty(), "{:?}", key_run.diagnostics);
    let Some(api::InterpValue::String(key)) = key_run.value() else {
        panic!(
            "source operation_key did not return text: {:?}",
            key_run.outcome
        )
    };
    let key = etas_host::StorageOperationKey::parse(key).unwrap();
    let after_key = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!((before_key + 7200..=after_key + 7200).contains(&key.expires_at()));
    let overflow = api::run_checked_blocking(
        &checked,
        EntryPoint { item },
        vec![api::InterpValue::Bool(true), api::InterpValue::Bool(false)],
        &host,
        options.clone(),
    )
    .unwrap();
    let api::RunOutcome::Failed(api::RunFailure::ExecutionFault(fault)) = overflow.outcome else {
        panic!("signed minimum must fault: {:?}", overflow.outcome);
    };
    assert_eq!(
        fault.code,
        etas_core::AnalysisDiagnosticCode::InvalidArguments
    );
    assert!(fault.message.contains("overflows the checked numeric type"));
    let crash = api::run_checked_blocking(
        &checked,
        EntryPoint { item },
        vec![api::InterpValue::Bool(false), api::InterpValue::Bool(true)],
        &host,
        options,
    )
    .unwrap();
    let api::RunOutcome::Failed(api::RunFailure::ExecutionFault(fault)) = crash.outcome else {
        panic!(
            "source assert must execute its abort primitive: {:?}",
            crash.outcome
        );
    };
    assert_eq!(
        fault.code,
        etas_core::AnalysisDiagnosticCode::ExecutionAborted
    );
    assert_eq!(fault.message, "assertion failed");

    std::fs::remove_dir_all(root).unwrap();
}
