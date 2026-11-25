use std::sync::Mutex;

use etas_frontend::{
    Frontend, ModulePath, ProjectEntry, ProjectSourceLoadOptions, ProjectSourceLoadScope,
    ProjectSourceLoader, SourceInput,
};
use etas_interpreter::{
    api::{self, EntryPoint, RunOptions, codec},
    host::{HostFuture, HostServiceAvailability, HostServices},
};

#[derive(Default)]
struct EntropyHost {
    unavailable: bool,
    requests: Mutex<Vec<etas_host::EntropyRequest>>,
    clock_requests: Mutex<Vec<etas_host::ClockRequest>>,
}

macro_rules! unused_host {
    ($($method:ident: $request:path => $response:path),* $(,)?) => {$(
        fn $method<'a>(&'a self, _: etas_host::execution::OperationContext, _: $request)
            -> HostFuture<'a, Result<$response, etas_host::HostError>> {
            panic!("unexpected {} host request", stringify!($method))
        }
    )*};
}

impl HostServices for EntropyHost {
    fn availability(&self) -> HostServiceAvailability {
        HostServiceAvailability {
            checkpoint: true,
            console: true,
            ..Default::default()
        }
    }
    fn entropy<'a>(
        &'a self,
        operation: etas_host::execution::OperationContext,
        request: etas_host::EntropyRequest,
    ) -> HostFuture<'a, Result<etas_host::EntropyResponse, etas_host::HostError>> {
        self.requests.lock().unwrap().push(request.clone());
        Box::pin(async move {
            if self.unavailable {
                return Err(etas_host::HostError::new(
                    etas_host::HostErrorCode::ProviderUnavailable,
                    "entropy provider unavailable",
                ));
            }
            etas_host::random::entropy(request, &operation).await
        })
    }
    fn clock<'a>(
        &'a self,
        operation: etas_host::execution::OperationContext,
        request: etas_host::ClockRequest,
    ) -> HostFuture<'a, Result<etas_host::ClockResponse, etas_host::HostError>> {
        self.clock_requests.lock().unwrap().push(request.clone());
        Box::pin(async move {
            if self.unavailable {
                return Err(etas_host::HostError::new(
                    etas_host::HostErrorCode::ProviderUnavailable,
                    "clock unavailable",
                ));
            }
            etas_host::clock::clock(request, &operation).await
        })
    }
    fn console<'a>(
        &'a self,
        _: etas_host::execution::OperationContext,
        _: etas_host::console::ConsoleRequest,
    ) -> HostFuture<'a, Result<etas_host::console::ConsoleResponse, etas_host::HostError>> {
        Box::pin(async {
            Err(etas_host::HostError::new(
                etas_host::HostErrorCode::IoFailure,
                "console input unavailable",
            )
            .with_detail("io_kind", "PermissionDenied"))
        })
    }
    unused_host! {
        compiler: etas_host::compiler::CompilerRequest => etas_host::compiler::CompilerResponse,
        model: etas_host::ModelRequest => etas_host::ModelResponse,
        tool: etas_host::ToolRequest => etas_host::ToolResponse,
        memory: etas_host::MemoryRequest => etas_host::MemoryResponse,
        memory_write: etas_host::memory::MemoryWriteRequest => etas_host::memory::MemoryWriteResponse,
        session: etas_host::SessionRequest => etas_host::SessionResponse,
        session_write: etas_host::session::SessionWriteRequest => etas_host::session::SessionWriteResponse,
        filesystem: etas_host::FilesystemRequest => etas_host::FilesystemResponse,
        command: etas_host::CommandRequest => etas_host::CommandResponse,
        command_process: etas_host::CommandProcessRequest => etas_host::CommandProcessResponse,
        tcp: etas_host::TcpConnectRequest => etas_host::TcpConnectResponse,
        stream: etas_host::StreamRequest => etas_host::StreamResponse,
        tls: etas_host::TlsConnectRequest => etas_host::TlsConnectResponse,
        secret: etas_host::SecretRequest => etas_host::SecretResponse,
        browser: etas_host::BrowserProtocolRequest => etas_host::BrowserProtocolResponse,
        approval: etas_host::ApprovalRequest => etas_host::ApprovalResponse,
        policy: etas_host::PolicyEvaluationRequest => etas_host::PolicyResponse,
    }
}

#[test]
fn source_entropy_and_clock_use_handlers_os_operations_and_retained_replay() {
    let root = etas_std::library::source_checkout_manifest()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut input = ProjectSourceLoader::default()
        .load(ProjectSourceLoadOptions {
            project_root: root.clone(),
            roots: vec![root.join("library")],
            entry: ProjectEntry {
                module: Some(ModulePath {
                    segments: vec!["app".into(), "entropy".into()],
                }),
                flow: "verify".into(),
            },
            scope: ProjectSourceLoadScope::FullSourceTree,
            source_kind: None,
            import_search_roots: vec![],
            external_module_roots: vec![],
        })
        .unwrap()
        .input;
    let mut source = SourceInput::anonymous(
        r#"
module app.entropy;
flow fixed_only() -> bytes ![] {
    let fixed = handler {
        std.effects.Random.entropy(count) => { resume bytes { data= [1, 2, 3] }; }
    };
    return handle { std.random.secure_bytes(3) } with fixed;
}
flow fixed_time() -> std.runtime.time.Time ![] {
    let fixed = handler {
        std.effects.Clock.now() => { resume std.runtime.time.Time { unix_millis= -17 }; }
    };
    return handle { std.runtime.time.now() } with fixed;
}
flow verify(check_failure: bool) -> (bytes, Option<std.runtime.time.Instant>) ![] {
    if check_failure {
        std.runtime.checkpoint("before-failure");
        let failed = match std.random.secure_bytes(32)? {
            Err(std.random.RandomError.Unavailable) => bytes { data= [] },
            _ => std.core.abort("entropy failure was not preserved"),
        };
        match std.runtime.time.now()? {
            Err(std.runtime.time.TimeError.Unavailable) => (),
            _ => std.core.abort("clock failure was not preserved"),
        };
        std.runtime.checkpoint("after-failure");
        return (failed, None);
    }
    match std.io.read_all()? {
        Err(std.runtime.error.IOError.PermissionDenied) => (),
        _ => std.core.abort("console error lost its shared identity"),
    };
    let chosen = fixed_only();
    std.core.assert(chosen == bytes { data= [1, 2, 3] });
    let unused = handler {
        std.effects.Random.entropy(count) => { std.core.abort("zero must not request entropy"); }
    };
    let empty = handle { std.random.secure_bytes(0) } with unused;
    std.core.assert(empty.length == 0);
    std.core.assert(fixed_time().unix_millis == -17);
    let no_sleep = handler {
        std.effects.Clock.sleep(duration) => { std.core.abort("zero must not request a timer"); }
    };
    handle { std.runtime.time.sleep(std.runtime.time.Duration.from_millis(0)) } with no_sleep;
    let start = match std.runtime.time.monotonic()? {
        Ok(instant) => instant,
        Err(_) => std.core.abort("monotonic clock unavailable"),
    };
    let pinned_clock = handler {
        std.effects.Clock.monotonic() => { resume start; }
    };
    let pinned = handle { std.runtime.time.monotonic() } with pinned_clock;
    match pinned.since(start) {
        Ok(duration) => std.core.assert(duration.as_millis() == 0),
        Err(_) => std.core.abort("same clock sample changed identity"),
    };
    std.runtime.checkpoint("before-entropy");
    let observed = match std.runtime.time.now()? {
        Ok(time) => time,
        Err(_) => std.core.abort("wall clock unavailable"),
    };
    std.core.assert(observed.unix_millis > 0);
    match std.runtime.time.sleep(std.runtime.time.Duration.from_millis(5))? {
        Ok(_) => (),
        Err(_) => std.core.abort("timer unavailable"),
    };
    let value = match std.random.secure_bytes(32)? {
        Ok(value) => value,
        Err(_) => std.core.abort("entropy unavailable"),
    };
    std.core.assert(value.length == 32);
    match std.runtime.time.elapsed(start)? {
        Ok(duration) => std.core.assert(duration.as_millis() >= 5),
        Err(_) => std.core.abort("elapsed clock unavailable"),
    };
    std.runtime.checkpoint("after-entropy");
    return (value, Some(start));
}

"#,
    );
    source.id = input
        .sources
        .iter()
        .max_by_key(|source| source.id.0)
        .unwrap()
        .id;
    source.id.0 += 1;
    input.sources.push(source);
    input.environment.local_source_package =
        Some(etas_code::PackageKey::from_name_and_version("std", "0.5.0").unwrap());
    let out = Frontend.check_standard_library_source(input);
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    let checked = out.checked.unwrap();
    let item = |name: &str| {
        checked
            .hir
            .items
            .iter()
            .find_map(|(id, item)| {
                let path = checked.hir.declaration_path(item.symbol()?)?;
                (path == ["app", "entropy", name]).then_some(id)
            })
            .unwrap()
    };
    let secure = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| {
            (checked.hir.declaration_path(item.symbol()?)? == ["std", "random", "secure_bytes"])
                .then_some(id)
        })
        .unwrap();
    assert_eq!(
        checked.effects.item_effects[&secure].determinism,
        etas_effects::Determinism::NonDeterministic
    );
    for name in ["now", "sleep", "monotonic", "elapsed"] {
        let item = checked
            .hir
            .items
            .iter()
            .find_map(|(id, item)| {
                (checked.hir.declaration_path(item.symbol()?)? == ["std", "runtime", "time", name])
                    .then_some(id)
            })
            .unwrap();
        assert_eq!(
            checked.effects.item_effects[&item].determinism,
            etas_effects::Determinism::NonDeterministic
        );
    }
    assert_ne!(
        checked.effects.item_effects[&item("fixed_only")].determinism,
        etas_effects::Determinism::NonDeterministic
    );
    assert!(
        checked.effects.item_effects[&item("fixed_only")]
            .default_actions
            .is_empty()
    );
    assert!(
        checked.effects.item_effects[&item("fixed_time")]
            .default_actions
            .is_empty()
    );
    assert_ne!(
        checked.effects.item_effects[&item("fixed_time")].determinism,
        etas_effects::Determinism::NonDeterministic
    );
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow("Random", "entropy"));
    for action in ["now", "sleep", "monotonic"] {
        options
            .host_context
            .authority
            .grants
            .push(etas_host::HostActionGrant::allow("Clock", action));
    }
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow(
            "Console",
            "stdin_read_all",
        ));
    let host = EntropyHost::default();
    let first = api::run_checked_blocking(
        &checked,
        EntryPoint {
            item: item("verify"),
        },
        vec![api::InterpValue::Bool(false)],
        &host,
        options.clone(),
    )
    .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert!(
        matches!(first.value(), Some(api::InterpValue::Tuple(values))
        if matches!(&values[0], api::InterpValue::Bytes(bytes) if bytes.len() == 32)),
        "unexpected runtime result: {:?}",
        first.value()
    );
    assert!(
        codec::value_json(first.value().unwrap()).is_err(),
        "Instant is not an ordinary serializable value"
    );
    assert_eq!(host.requests.lock().unwrap().len(), 1);
    assert_eq!(host.clock_requests.lock().unwrap().len(), 4);
    assert_eq!(first.checkpoints.len(), 2);
    let mut replay = first.checkpoints[0].clone();
    replay.completed_host_boundaries = first.checkpoints[1].completed_host_boundaries.clone();
    let artifact = codec::checkpoint_artifact_json(&[], "verify", &replay).unwrap();
    let restored = codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let unavailable = EntropyHost {
        unavailable: true,
        ..Default::default()
    };
    let resumed =
        api::resume_checkpoint_blocking(&checked, &restored, &unavailable, options.clone())
            .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    assert!(unavailable.requests.lock().unwrap().is_empty());
    assert!(unavailable.clock_requests.lock().unwrap().is_empty());
    let failed = api::run_checked_blocking(
        &checked,
        EntryPoint {
            item: item("verify"),
        },
        vec![api::InterpValue::Bool(true)],
        &unavailable,
        options.clone(),
    )
    .unwrap();
    assert!(failed.diagnostics.is_empty(), "{:?}", failed.diagnostics);
    assert!(
        matches!(failed.value(), Some(api::InterpValue::Tuple(values))
        if matches!(&values[0], api::InterpValue::Bytes(bytes) if bytes.is_empty())),
        "unexpected failure result: {:?}",
        failed.value()
    );
    assert_eq!(unavailable.requests.lock().unwrap().len(), 1);
    assert_eq!(unavailable.clock_requests.lock().unwrap().len(), 1);
    let mut failed_replay = failed.checkpoints[0].clone();
    failed_replay.completed_host_boundaries =
        failed.checkpoints[1].completed_host_boundaries.clone();
    let replayed_failure =
        api::resume_checkpoint_blocking(&checked, &failed_replay, &host, options).unwrap();
    assert!(
        replayed_failure.diagnostics.is_empty(),
        "{:?}",
        replayed_failure.diagnostics
    );
    assert_eq!(replayed_failure.value(), failed.value());
    assert_eq!(host.requests.lock().unwrap().len(), 1);
    assert_eq!(host.clock_requests.lock().unwrap().len(), 4);
}
