use super::super::*;

#[derive(Debug, Default)]
struct ManagedAdmissionEvents {
    events: std::sync::Mutex<Vec<String>>,
    reads: std::sync::atomic::AtomicUsize,
    changed: tokio::sync::Notify,
}
impl crate::api::RunEventObserver for ManagedAdmissionEvents {
    fn observe(&self, event: &WorkflowEvent) {
        match event {
            WorkflowEvent::AgentRequestSettled {
                committed: false, ..
            } => {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            WorkflowEvent::ScopeSerialized { reason, .. } => {
                self.events.lock().unwrap().push(reason.clone())
            }
            _ => {}
        }
        self.changed.notify_one();
    }
}
impl ManagedAdmissionEvents {
    async fn wait_for_reads(&self, count: usize) {
        loop {
            let changed = self.changed.notified();
            if self.reads.load(std::sync::atomic::Ordering::SeqCst) >= count {
                return;
            }
            changed.await;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_native_approval_restores_the_handler_and_discards_agent_updates() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.{println, read_line};
import std.task.{run, race, Scheduler};
effect Provider { action note() -> unit; }
agent Counter = { n: i32 };
impl Counter {
    public flow increment(var self) -> i32 ![Provider] {
        self.n = self.n + 1;
        perform Provider.note();
        return self.n;
    }
    public flow get(self) -> i32 ![] { return self.n; }
}
flow main() -> i32 {
    let counter=Counter { n=0 };
    let winner=handle {
        run((scheduler: Scheduler) => race([
            () => { read_line()?; return 7; },
            () => counter.increment(),
        ]))
    } with {
        Provider.note()=> { println("pending"); resume (); }
    };
    return winner + counter.get();
}
"#,
    );
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
    ]));
    let approval = host.pause_approval_completion();
    let console = host.pause_console_completion();
    host.seed_policy_decision_for(
        "Console.stdout_write",
        PolicyDecision::RequireApproval {
            request: ApprovalRequest {
                id: HostRequestId(1907),
                reason: "native write approval".into(),
                requested_grants: vec![HostActionGrant::allow("Console", "stdout_write")],
                trace: TraceContext::root(TraceId(307)),
            },
        },
    );
    let mut options = RunOptions::default();
    options.host_context.trace = TraceContext::root(TraceId(307));
    options.host_context.authority.grants = vec![
        HostActionGrant::allow("Console", "stdin_read_line"),
        HostActionGrant::allow("Console", "stdout_write"),
    ];
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("native-policy".into()));
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        options,
    );
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        tokio::join!(invocation.execute(), async {
            approval.started.notified().await;
            console.release.add_permits(1);
        })
    })
    .await
    .expect("cancelled native authorization must settle and restore the Handler owner");
    let result = result.unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    assert_eq!(
        host.console_call_count(),
        1,
        "the unapproved stdout write never dispatches"
    );
    assert_eq!(host.stdout_text(), "");
}

#[tokio::test(flavor = "current_thread")]
async fn managed_policy_and_approval_waits_allow_peer_requests_to_run() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.task.{run, start, await, Scheduler};
agent Counter = { n: i32 };
impl Counter {
    public flow increment(var self) -> i32 {
        println("writer");
        self.n = self.n + 1;
        return self.n;
    }
    public flow get(self) -> i32 ![] { return self.n; }
}
flow main() -> i32 {
    let counter = Counter { n = 0 };
    let policy_phase = run((scheduler: Scheduler) => {
        let writer = start(() => counter.increment());
        let peer = start(() => { return counter.get(); });
        return await(writer) + await(peer);
    });
    let approval_phase = run((scheduler: Scheduler) => {
        let writer = start(() => counter.increment());
        let peer = start(() => { return counter.get(); });
        return await(writer) + await(peer);
    });
    return policy_phase + approval_phase;
}
"#,
    );
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
    ]));
    let policy = host.pause_policy_completion("Agentic.call");
    let approval = host.pause_approval_completion();
    host.seed_policy_decision_for("Agentic.call", PolicyDecision::Allow);
    host.seed_policy_decision_for("Agentic.call", PolicyDecision::Allow);
    host.seed_policy_decision_for(
        "Agentic.call",
        PolicyDecision::RequireApproval {
            request: ApprovalRequest {
                id: HostRequestId(1903),
                reason: "approve second update".into(),
                requested_grants: vec![HostActionGrant::allow("Console", "stdout_write")],
                trace: TraceContext::root(TraceId(303)),
            },
        },
    );
    let events = std::sync::Arc::new(ManagedAdmissionEvents::default());
    let mut options = RunOptions::default();
    options.event_observer = Some(events.clone());
    options.host_context.trace = TraceContext::root(TraceId(303));
    options.host_context.authority.grants = vec![HostActionGrant::allow("Console", "stdout_write")];
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("managed-policy".into()));
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        options,
    );
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        tokio::join!(invocation.execute(), async {
            policy.started.notified().await;
            events.wait_for_reads(1).await;
            policy.release.add_permits(1);
            approval.started.notified().await;
            events.wait_for_reads(2).await;
            approval.release.add_permits(1);
        })
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "managed admission must release the scheduler: stdout={:?}, approvals={}, policy={:?}, events={:?}",
            host.stdout_text(),
            host.approval_call_count(),
            host.policy_requests()
                .iter()
                .map(|request| &request.subject)
                .collect::<Vec<_>>(), events.events.lock().unwrap()
        )
    });
    let result = result.unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(4)));
    let console = host
        .policy_requests()
        .into_iter()
        .filter(|request| request.subject.kind == "console")
        .collect::<Vec<_>>();
    assert_eq!(console.len(), 2);
    assert!(
        console[..1]
            .iter()
            .all(|request| request.authority.approvals.is_empty())
    );
    assert_eq!(console[1].authority.approvals.len(), 1);
    assert_eq!(console[1].authority.approvals[0].id, HostRequestId(1903));
}

#[tokio::test(flavor = "current_thread")]
async fn native_policy_and_approval_waits_keep_agent_updates_isolated() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.task.{run, start, await, Scheduler};
agent Counter = { n: i32 };
impl Counter {
    public flow increment(var self) -> i32 {
        self.n = self.n + 1;
        println("writer");
        return self.n;
    }
    public flow get(self) -> i32 ![] { return self.n; }
}
flow main() -> i32 {
    let counter = Counter { n = 0 };
    let policy_phase = run((scheduler: Scheduler) => {
        let writer = start(() => counter.increment());
        let peer = start(() => { return counter.get(); });
        return await(writer) + await(peer);
    });
    let approval_phase = run((scheduler: Scheduler) => {
        let writer = start(() => counter.increment());
        let peer = start(() => { return counter.get(); });
        return await(writer) + await(peer);
    });
    return policy_phase + approval_phase;
}
"#,
    );
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
    ]));
    let policy = host.pause_policy_completion("Console.stdout_write");
    let approval = host.pause_approval_completion();
    host.seed_policy_decision_for("Console.stdout_write", PolicyDecision::Allow);
    host.seed_policy_decision_for(
        "Console.stdout_write",
        PolicyDecision::RequireApproval {
            request: ApprovalRequest {
                id: HostRequestId(1906),
                reason: "approve second update".into(),
                requested_grants: vec![HostActionGrant::allow("Console", "stdout_write")],
                trace: TraceContext::root(TraceId(306)),
            },
        },
    );
    let events = std::sync::Arc::new(ManagedAdmissionEvents::default());
    let mut options = RunOptions::default();
    options.event_observer = Some(events.clone());
    options.host_context.trace = TraceContext::root(TraceId(306));
    options.host_context.authority.grants = vec![HostActionGrant::allow("Console", "stdout_write")];
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("managed-policy".into()));
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        options,
    );
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        tokio::join!(invocation.execute(), async {
            policy.started.notified().await;
            events.wait_for_reads(1).await;
            policy.release.add_permits(1);
            approval.started.notified().await;
            events.wait_for_reads(2).await;
            approval.release.add_permits(1);
        })
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "managed admission must release the scheduler: stdout={:?}, approvals={}, policy={:?}, events={:?}",
            host.stdout_text(),
            host.approval_call_count(),
            host.policy_requests()
                .iter()
                .map(|request| &request.subject)
                .collect::<Vec<_>>(), events.events.lock().unwrap()
        )
    });
    let result = result.unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(4)));
    let console = result
        .events
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted {
                kind: etas_host::HostRequestKind::Console,
                authority,
                ..
            }) => Some(authority.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(console.len(), 2);
    assert!(
        console[..1]
            .iter()
            .all(|request| request.approvals.is_empty())
    );
    assert_eq!(console[1].approvals.len(), 1);
    assert_eq!(console[1].approvals[0].id, HostRequestId(1906));
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_managed_approval_does_not_admit_the_request_or_stop_its_peer() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.read_line;
import std.task.{run, race, Scheduler};
agent Counter = { n: i32 };
impl Counter {
    public flow increment(var self) -> i32 ![] { self.n = self.n + 1; return self.n; }
    public flow get(self) -> i32 ![] { return self.n; }
}
flow main() -> i32 {
    let counter=Counter { n=0 };
    let winner=run((scheduler: Scheduler) => race([
        () => { read_line()?; return 7; },
        () => counter.increment(),
    ]));
    return winner + counter.get();
}
"#,
    );
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
    ]));
    let approval = host.pause_approval_completion();
    let console = host.pause_console_completion();
    host.seed_policy_decision_for(
        "Agentic.call",
        PolicyDecision::RequireApproval {
            request: ApprovalRequest {
                id: HostRequestId(1904),
                reason: "approval remains pending".into(),
                requested_grants: vec![],
                trace: TraceContext::root(TraceId(304)),
            },
        },
    );
    let mut options = RunOptions::default();
    options.host_context.trace = TraceContext::root(TraceId(304));
    options.host_context.authority.grants =
        vec![HostActionGrant::allow("Console", "stdin_read_line")];
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("managed-policy".into()));
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        options,
    );
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        tokio::join!(invocation.execute(), async {
            approval.started.notified().await;
            console.release.add_permits(1);
        })
    })
    .await
    .expect("a cancelled approval and its native operation must settle");
    let result = result.unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    let queued = result
        .events
        .iter()
        .filter(|event| matches!(event, WorkflowEvent::AgentRequestQueued { .. }))
        .count();
    assert_eq!(
        queued, 1,
        "only the surviving read request reaches the mailbox"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn managed_agent_policy_runs_before_initialization_and_mailbox_admission() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
agent Counter = { n: i32 };
impl Counter {
    public flow increment(var self) -> i32 {
        println("body")?;
        self.n = self.n + 1;
        return self.n;
    }
}
flow initial() -> i32 { println("initial")?; return 0; }
flow main() -> i32 {
    let counter = Counter { n = initial() };
    return counter.increment();
}
"#,
    );
    for (action, writes) in [("Agentic.create", 0), ("Agentic.call", 1)] {
        let mut availability = availability(&[HostRequirementKind::Console]);
        availability.policy = true;
        availability.host_authority = true;
        let host = FakeHost::new(availability);
        host.seed_policy_decision_for(
            action,
            PolicyDecision::Deny {
                reason: "managed boundary denied".into(),
            },
        );
        let mut options = RunOptions::default();
        options.host_context.authority.grants =
            vec![HostActionGrant::allow("Console", "stdout_write")];
        options.host_context.authority.policy =
            boundary_policy_context(HostValue::String("managed-policy".into()));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                options,
            )
            .await
            .unwrap();
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("managed boundary denied")),
            "{:?}",
            result.diagnostics
        );
        assert_eq!(host.console_call_count(), writes);
        assert!(
            !result
                .events
                .iter()
                .any(|event| matches!(event, WorkflowEvent::AgentRequestQueued { .. }))
        );
        let requests = host.policy_requests();
        let request = requests
            .iter()
            .find(|request| {
                request.subject.attributes.iter().any(|(name, value)| {
                    name == "qualified_action" && value == &HostValue::String(action.into())
                })
            })
            .unwrap();
        assert_eq!(request.subject.kind, "agentic");
        if action == "Agentic.create" {
            assert!(
                !result
                    .events
                    .iter()
                    .any(|event| matches!(event, WorkflowEvent::AgentCreated { .. }))
            );
        } else {
            assert!(
                request
                    .subject
                    .attributes
                    .iter()
                    .any(|(name, _)| name == "method_symbol")
            );
            assert!(
                request
                    .subject
                    .attributes
                    .iter()
                    .any(|(name, _)| name == "request")
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_agent_model_request_retains_inherited_bounds_after_annotations() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Tokens;
effect Provider { action answer() -> string; }
agent Writer = {};
impl Writer {
    public flow answer(self) -> string {
        return perform Provider.answer();
    }
}
@limits([Tokens(31)])
agent Gateway = {};
impl Gateway {
    public flow answer(self) -> string {
        let writer = Writer {};
        return handle { writer.answer() } with {
            Provider.answer() => {
                resume perform infer(Prompt.new().user(Public("hello")));
            }
        };
    }
}
flow main() -> string {
    let gateway = Gateway {};
    return gateway.answer();
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let mut options = RunOptions::default();
    options.host_context.authority.grants = vec![
        HostActionGrant::allow("Agentic", "infer"),
        HostActionGrant::allow("Agentic", "step"),
    ];
    options.model_policy.provider_capabilities = Some(full_model_capabilities());
    options.model_policy.max_tool_rounds = 0;
    options.model_policy.options.max_output_tokens = Some(7);
    options.model_policy.budget = Some(etas_host::Budget {
        tokens: Some(etas_host::TokenBudget { max_tokens: 9 }),
        ..Default::default()
    });
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("draft".into())));
    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].options.max_output_tokens, Some(7));
    let account = requests[0].budget.account_snapshots().unwrap();
    assert_eq!(
        account.len(),
        1,
        "caller and owner views of the same account are charged once"
    );
    assert_eq!(account[0].consumed_tokens, 2);
    assert_eq!(requests[0].budget.remaining_tokens().unwrap(), Some(7));
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_an_agent_request_stops_its_delegated_host_operation_and_restores_the_owner() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.{println, read_line};
import std.task.{run, race, Scheduler, Async};
effect Provider { action value(id: i32) -> i32; }
agent Service = {};
impl Service {
    public flow answer(self, id: i32) -> i32 ![Provider] {
        return perform Provider.value(id);
    }
}
flow main() -> i32 {
    let service = Service {};
    return handle {
        run((scheduler: Scheduler) => {
            let winner = race([
                () => { read_line()?; return 7; },
                () => { return service.answer(1); },
            ]);
            return winner + service.answer(2) + service.answer(3);
        })
    } with {
        Provider.value(id) => {
            if id == 1 {
                println("first")?;
                while true { perform Async.yield(); }
            }
            println("next")?;
            resume id;
        }
    };
}
"#,
    );
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let gate = host.pause_console_completion();
    gate.pending_calls
        .store(2, std::sync::atomic::Ordering::SeqCst);
    let mut options = RunOptions::default();
    options.host_context.authority.grants = vec![
        HostActionGrant::allow("Console", "stdin_read_line"),
        HostActionGrant::allow("Console", "stdout_write"),
    ];
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        options,
    );
    let (result, cancelled) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(invocation.execute(), async {
            while host.console_call_count() < 2 {
                gate.started.notified().await;
            }
            // The first admitted console wait is the race winner's stdin.
            // Keep the Handler's stdout reply pending while cancellation runs.
            gate.release.add_permits(1);
            let signal = gate.signals.lock().unwrap()[1].clone();
            let cancelled =
                tokio::time::timeout(std::time::Duration::from_secs(1), signal.cancelled())
                    .await
                    .is_ok();
            gate.release.add_permits(2);
            cancelled
        })
    })
    .await
    .expect("cancelled request and its Host operation must join");
    let result = result.unwrap();
    assert!(
        cancelled,
        "the delegated native operation belongs to the cancelled request"
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(12)));
    assert_eq!(host.stdout_text(), "first\nnext\nnext\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_shared_outer_handler_keeps_native_approvals_in_each_agent_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.task.{run, start, await, Scheduler};
effect Provider { action value(id: i32) -> i32; }
agent Service = {};
impl Service {
    public flow answer(self, id: i32) -> i32 ![Provider] {
        return perform Provider.value(id);
    }
}
flow main() -> i32 {
    let service = Service {};
    return handle {
        run((scheduler: Scheduler) => {
            let first = start(() => service.answer(1));
            let second = start(() => service.answer(2));
            return await(first) + await(second);
        })
    } with {
        Provider.value(id) => { println("request"); resume id; }
    };
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
    ]));
    host.seed_policy_decision_for(
        "Console.stdout_write",
        PolicyDecision::RequireApproval {
            request: ApprovalRequest {
                id: HostRequestId(907),
                reason: "first request approval".into(),
                requested_grants: vec![HostActionGrant::allow("Console", "stdout_write")],
                trace: TraceContext::root(TraceId(94)),
            },
        },
    );
    let mut options = RunOptions::default();
    options.host_context.trace = TraceContext::root(TraceId(94));
    options.host_context.authority.grants = vec![HostActionGrant::allow("Console", "stdout_write")];
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("request-policy".into()));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(3)));
    let requests = host
        .policy_requests()
        .into_iter()
        .filter(|request| request.subject.kind == "console")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].authority.approvals.is_empty());
    assert!(requests[1].authority.approvals.is_empty());
    assert_eq!(host.console_call_count(), 2);
}

#[test]
fn plan_collects_host_requirements_for_agent_entry() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let plan = Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let Some(InterpreterSupport::RequiresHost(requirements)) = plan.host_requirements.entry else {
        panic!("expected host requirement for agent inference");
    };
    assert!(requirements.kinds.contains(&HostRequirementKind::Agentic));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_missing_host_handler_for_reachable_host_requirement() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hi".to_owned().into())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingHostHandler)
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_spec_method_selection() {
    let checked = checked_project(
        r#"
module app.main;

spec PromptEncode {
  flow encode(self) -> string;
}

impl string ~ PromptEncode {
  flow encode(self) -> string {
    return "encoded";
  }
}

flow main(input: string) -> string {
  return input::PromptEncode.encode();
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hi".to_owned().into())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("encoded".to_owned().into())),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn generic_flow_dispatches_spec_method_with_checked_type_arguments() {
    let checked = checked_project(
        r#"
module app.main;
spec Identity<T> { flow id(input: T) -> T; }
type Token = i32;
impl Token ~ Identity<Token> {
  flow id(input: Token) -> Token { return input; }
}
flow apply<T ~ Identity<T>>(input: T) -> T {
  return input::Identity<T>.id();
}
flow main() -> bool {
  return apply(Token(7)) == Token(7);
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::Bool(true)),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_agent_call_through_model_host_service() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return perform infer(Prompt.new().system(Trusted("write concise output")).user(Public(input)));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(42)),
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(TokenBudget { max_tokens: 128 }),
                ..Budget::default()
            }),
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("draft".to_owned().into()))
    );
    assert_eq!(host.model_call_count(), 1);

    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages.len(), 2);
    assert_eq!(requests[0].messages[0].role, ModelRole::System);
    assert_eq!(
        requests[0].messages[0].content,
        vec![ModelContent::Text("write concise output".to_owned())]
    );
    assert_eq!(requests[0].messages[1].role, ModelRole::User);
    assert_eq!(
        requests[0].messages[1].content,
        vec![ModelContent::Text("hello".to_owned())]
    );
    assert_eq!(
        requests[0].authority.grants,
        vec![HostActionGrant::allow("Agentic", "infer")]
    );
    assert_eq!(requests[0].trace, TraceContext::root(TraceId(42)));
    assert_eq!(
        requests[0].budget.limits().tokens,
        Some(TokenBudget { max_tokens: 128 })
    );
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted {
            id,
            kind: etas_host::HostRequestKind::Model,
            metadata,
            authority,
            trace,
            started_at_unix_micros,
        }) if *id == requests[0].id && authority.grants == vec![HostActionGrant::allow("Agentic", "infer")]
            && trace == &TraceContext::root(TraceId(42))
            && metadata.qualified_action == requests[0].action.qualified_name()
            && metadata.payload_digest.len() == 64
            && *started_at_unix_micros > 0
    )));
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished {
            id,
            outcome: etas_host::HostOutcome::Succeeded,
            finished_at_unix_micros,
            ..
        }) if *id == requests[0].id && *finished_at_unix_micros > 0
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn model_response_id_mismatch_fails_closed_and_preserves_trace_pairing() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main() -> string {
  return Writer.run("hello");
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    host.force_next_model_response_id(HostRequestId(99));
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
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(91)),
                    budget: etas_host::ExecutionBudget::start(Budget::default()),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.value().cloned().is_none());
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("host response id does not match the originating request id")
    }));
    let trace_ids = result
        .events
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted { id, .. })
            | WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished { id, .. }) => {
                Some(*id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let request = host.model_requests()[0].id;
    assert_eq!(trace_ids, vec![request, request]);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished {
            outcome: etas_host::HostOutcome::Failed(error),
            ..
        }) if error.code == HostErrorCode::InvalidResponse
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn run_owned_token_budget_is_consumed_across_model_calls() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main() -> string {
  let first = Writer.run("first");
  return Writer.run(first);
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("one");
    host.seed_model_response_text("two");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(43)),
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(TokenBudget { max_tokens: 3 }),
                ..Budget::default()
            }),
        },
        ..RunOptions::default()
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.value().cloned().is_none());
    assert_eq!(host.model_call_count(), 2);
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("run-owned execution token budget is exhausted")
        }),
        "{:?}",
        result.diagnostics
    );
    let snapshot = host.model_requests()[0]
        .budget
        .snapshot()
        .expect("shared budget snapshot");
    assert_eq!(snapshot.consumed_tokens, 2);
    assert_eq!(snapshot.reserved_tokens, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn exhausted_token_budget_blocks_model_dispatch_before_the_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main() -> string {
  let first = Writer.run("first");
  return Writer.run(first);
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("one");
    host.seed_model_response_text("must not run");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(143)),
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(TokenBudget { max_tokens: 2 }),
                ..Budget::default()
            }),
        },
        ..RunOptions::default()
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.value().is_none());
    assert_eq!(host.model_call_count(), 1);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("run-owned execution token budget is exhausted")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn initially_zero_token_budget_never_dispatches_a_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
agent Writer() -> string { return perform infer(Prompt.new()); }
flow main() -> string { return Writer.run(); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("must not run");
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Agentic", "infer"));
    options.host_context.budget = etas_host::ExecutionBudget::start(Budget {
        tokens: Some(TokenBudget { max_tokens: 0 }),
        ..Budget::default()
    });

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.value().is_none());
    assert_eq!(host.model_call_count(), 0);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("run-owned execution token budget is exhausted")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn token_budget_fails_closed_when_model_omits_usage() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main() -> string {
  return Writer.run("first");
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text_with_usage("one", None);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(44)),
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(TokenBudget { max_tokens: 10 }),
                ..Budget::default()
            }),
        },
        ..RunOptions::default()
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.value().cloned().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains(
                "model response omitted usage required by the execution token or cost limits",
            )
        }),
        "{:?}",
        result.diagnostics
    );
    let snapshot = host.model_requests()[0]
        .budget
        .snapshot()
        .expect("shared budget snapshot");
    assert_eq!(snapshot.consumed_tokens, 0);
    assert_eq!(snapshot.reserved_tokens, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn model_cost_usage_is_settled_into_the_run_owned_budget() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main() -> string {
  return Writer.run("first");
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text_with_usage(
        "one",
        Some(etas_host::ModelUsage {
            input_tokens: Some(1),
            output_tokens: Some(1),
            cost: Some(etas_host::ModelCostUsage {
                micros: 30,
                currency: "USD".to_owned(),
            }),
        }),
    );
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(45)),
            budget: etas_host::ExecutionBudget::start(Budget {
                cost: Some(etas_host::CostBudget {
                    max_micros: 100,
                    currency: "USD".to_owned(),
                }),
                ..Budget::default()
            }),
        },
        ..RunOptions::default()
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("one".to_owned().into())),
        "{:?}",
        result.diagnostics
    );
    let snapshot = host.model_requests()[0]
        .budget
        .snapshot()
        .expect("shared budget snapshot");
    assert_eq!(snapshot.consumed_cost_micros, 30);
    assert_eq!(snapshot.reserved_cost_micros, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_agent_prompt_body_after_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.io.read_line;

agent Writer() -> string ![Error<IOError>] {
  let line = read_line();
  return perform infer(Prompt.new().user(Public(line)));
}

flow main() -> string ![Error<IOError>] {
  return Writer.run();
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Console,
    ]));
    host.seed_stdin("from stdin\n");
    host.seed_model_response_text("model answer");

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
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(52)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("model answer".to_owned().into()))
    );
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(host.model_call_count(), 1);

    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages.len(), 1);
    assert_eq!(requests[0].messages[0].role, ModelRole::User);
    assert_eq!(
        requests[0].messages[0].content,
        vec![ModelContent::Text("from stdin\n".to_owned())]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_prompt_method_argument_after_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.io.read_line;

agent Writer() -> string ![Error<IOError>] {
  return perform infer(Prompt.new().user(Public(read_line())));
}

flow main() -> string ![Error<IOError>] {
  return Writer.run();
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Console,
    ]));
    host.seed_stdin("nested stdin\n");
    host.seed_model_response_text("nested answer");

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
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(53)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String(
            "nested answer".to_owned().into()
        ))
    );
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(host.model_call_count(), 1);

    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages.len(), 1);
    assert_eq!(requests[0].messages[0].role, ModelRole::User);
    assert_eq!(
        requests[0].messages[0].content,
        vec![ModelContent::Text("nested stdin\n".to_owned())]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_local_method_argument_after_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.read_line;

flow main() -> bool ![Error<IOError>] {
  let empty: Array<string> = [];
  let values = empty.push(read_line());
  return values.is_empty();
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.seed_stdin("from method\n");

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
        Some(value::InterpValue::Bool(false))
    );
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_agent_model_tools_annotations_to_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

tool Search() -> string {
  return "local";
}

@model(model = "local-qwen")
@tools([Search])
agent Writer(input: string) -> string ![] {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(44)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].model,
        etas_host::ModelName("local-qwen".to_owned())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_agent_annotations_to_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Tokens;

tool Search() -> string {
  return "local";
}

@model("annotated-model", adapter = "mock-provider")
@tools([Search])
@limits([Tokens(31)])
@trace(VirtualStages([Logical, Search]))
agent Writer(input: string) -> string ![] {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(144)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let request = requests.first().expect("model request should be sent");
    assert_eq!(
        request.provider,
        Some(ModelProviderId("mock-provider".to_owned()))
    );
    assert_eq!(
        request.model,
        etas_host::ModelName("annotated-model".to_owned())
    );
    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.options.max_output_tokens, None);
    assert_eq!(
        request.budget.limits().tokens,
        Some(TokenBudget { max_tokens: 31 })
    );
    assert!(
        request.options.metadata.is_empty(),
        "@trace must stay in interpreter trace metadata, not provider request metadata"
    );
    assert!(
        result.events.iter().any(|event| {
            matches!(
                event,
                WorkflowEvent::AgentTracePlan { trace, .. }
                    if trace == &vec!["VirtualStages([Logical, Search])".to_owned()]
            )
        }),
        "agent @trace annotation should produce an internal trace plan event: {:?}",
        result.events
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_keeps_locked_runtime_model_over_agent_model_annotation() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

@model(model = "fixture-model")
agent Writer(input: string) -> string ![] {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(45)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            model: etas_host::ModelName("runtime-model".to_owned()),
            model_locked: true,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].model,
        etas_host::ModelName("runtime-model".to_owned())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_agent_limits_annotation_to_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Tokens;

@limits([Tokens(17)])
agent Writer(input: string) -> string ![] {
  return perform infer(Prompt.new().user(Public(input)));
}


flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let request = requests.first().expect("model request should be sent");
    assert_eq!(request.options.max_output_tokens, None);
    assert_eq!(
        request.budget.limits().tokens,
        Some(TokenBudget { max_tokens: 17 })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_pipeline_caller_limits_to_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Tokens;

agent Writer = {};
impl Writer {
  public flow run(self, input: string) -> string {
    return perform infer(Prompt.new().user(Public(input)));
  }
}

@limits([Tokens(23)])
flow main(input: string) -> string {
  return input ~> (Writer {}).run;
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let request = requests.first().expect("model request should be sent");
    assert_eq!(request.options.max_output_tokens, None);
    assert_eq!(
        request.budget.limits().tokens,
        Some(TokenBudget { max_tokens: 23 })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resumed_pipeline_target_selection_preserves_the_callers_checked_limits() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.checkpoint;
import std.runtime.limits.Tokens;
agent Writer = {};
impl Writer {
  public flow run(self, input: string) -> string {
    return perform infer(Prompt.new().user(Public(input)));
  }
}
flow choose() -> string -> string ![] {
    checkpoint("select target");
    return (Writer {}).run;
}
@limits([Tokens(23)])
flow main() -> string { return "hello" ~> choose(); }
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    host.seed_model_response_text("draft");
    let first = Interpreter
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
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.checkpoints.len(), 1);
    let artifact =
        api::codec::checkpoint_artifact_json(&[], "main", &first.checkpoints[0]).unwrap();
    let checkpoint = api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed_host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    resumed_host.seed_model_response_text("resumed");
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &resumed_host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value(),
        Some(&InterpValue::String("resumed".into()))
    );
    assert!(resumed.checkpoints.is_empty());
    let requests = resumed_host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].options.max_output_tokens, None);
    assert_eq!(
        requests[0].budget.limits().tokens,
        Some(TokenBudget { max_tokens: 23 })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_decodes_typed_agent_record_output_from_json() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

type Draft = { title: string, score: i64 };

@model(model = "local-qwen")
agent Writer(input: string) -> Draft ![] {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> Draft {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text(r#"{"title":"Hello","score":7}"#);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(45)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let schema = requests
        .first()
        .and_then(|request| request.response_schema.as_ref())
        .expect("typed agent request should carry an output schema");
    match schema {
        HostSchema::Record(fields) => {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].name, "title");
            assert_eq!(fields[0].schema, HostSchema::String);
            assert_eq!(fields[1].name, "score");
            assert_eq!(fields[1].schema, HostSchema::Int);
        }
        other => panic!("expected record output schema, got {other:?}"),
    }
    let Some(value::InterpValue::Nominal { value, .. }) = result.value().cloned() else {
        panic!("typed agent output must preserve nominal runtime identity");
    };
    assert_eq!(
        *value,
        value::InterpValue::Record(
            vec![
                (
                    "title".to_owned(),
                    value::InterpValue::String("Hello".to_owned().into())
                ),
                ("score".to_owned(), value::InterpValue::i64(7)),
            ]
            .into()
        )
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_lowers_and_decodes_typed_agent_enum_output() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

enum Choice {
  Read(string),
  Final(string),
}

agent Choose() -> Choice ![] {
  return perform infer<Choice>(Prompt.new().system(Trusted("Choose a file.")));
}

flow main() -> Choice {
  return Choose.run();
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text(r#"{"name":"Read","fields":["note.txt"]}"#);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(46)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let schema = host.model_requests()[0]
        .response_schema
        .clone()
        .expect("typed enum request should carry an output schema");
    assert_eq!(
        schema,
        HostSchema::Variant(vec![
            etas_host::HostVariantSchema {
                name: "Read".to_owned(),
                fields: vec![HostSchema::String],
            },
            etas_host::HostVariantSchema {
                name: "Final".to_owned(),
                fields: vec![HostSchema::String],
            },
        ])
    );
    let Some(value::InterpValue::Nominal { value, .. }) = result.value().cloned() else {
        panic!("typed enum output must preserve nominal runtime identity");
    };
    assert_eq!(
        *value,
        value::InterpValue::Variant {
            name: "Read".into(),
            fields: vec![value::InterpValue::String("note.txt".into())].into(),
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_typed_agent_json_decode_context() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

type Draft = { title: string, score: i64 };

agent Writer(input: string) -> Draft ![] {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> Draft {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("not json output");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(46)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider: Some(ModelProviderId("mock-provider".to_owned())),
            provider_capabilities: Some(full_model_capabilities()),
            model: ModelName("mock-model".to_owned()),
            max_tool_rounds: 0,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    let diagnostic_text = result
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        diagnostic_text.contains("mock-provider/mock-model"),
        "{diagnostic_text}"
    );
    assert!(diagnostic_text.contains("Record"), "{diagnostic_text}");
    assert!(diagnostic_text.contains("text"), "{diagnostic_text}");
    assert!(
        diagnostic_text.contains("not json output"),
        "{diagnostic_text}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_repairs_invalid_typed_agent_json_output() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

type Draft = { title: string, score: i64 };

agent Writer(input: string) -> Draft ![] {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> Draft {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("not json output");
    host.seed_model_response_text(r#"{"title":"Fixed","score":9}"#);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(47)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider: Some(ModelProviderId("mock-provider".to_owned())),
            provider_capabilities: Some(full_model_capabilities()),
            model: ModelName("mock-model".to_owned()),
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.model_call_count(), 2);
    let Some(value::InterpValue::Nominal { value, .. }) = result.value().cloned() else {
        panic!("repaired typed output must preserve nominal runtime identity");
    };
    assert_eq!(
        *value,
        value::InterpValue::Record(
            vec![
                (
                    "title".to_owned(),
                    value::InterpValue::String("Fixed".to_owned().into())
                ),
                ("score".to_owned(), value::InterpValue::i64(9)),
            ]
            .into()
        )
    );
    let requests = host.model_requests();
    let repair_request = requests.get(1).expect("repair model request");
    assert!(
        repair_request.messages.iter().any(|message| {
            message.role == ModelRole::User
                && message.content.iter().any(|content| {
                    matches!(
                        content,
                        ModelContent::Text(text)
                            if text.contains("previous response did not satisfy")
                                && text.contains("valid JSON")
                    )
                })
        }),
        "{repair_request:#?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_typed_agent_integer_out_of_range() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

@model(model = "local-qwen")
agent Writer(input: string) -> i8 ![] {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> i8 {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("200");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(47)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(result.value().cloned(), None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_typed_agent_trusted_output_from_model_json() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

@model(model = "local-qwen")
agent Writer(input: string) -> Trusted<string> ![] {
  return perform infer(Prompt.new().user(Public(input)));
}

flow main(input: string) -> Trusted<string> {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text(r#""draft""#);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(48)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned().into())],
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(result.value().cloned(), None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn type_stages_construct_a_fresh_agent_for_each_pipeline_invocation() {
    let checked = checked_project(
        r#"
module app.main;
agent Echo = {};
impl Echo {
    public flow run<T>(self, value: T) -> T { return value; }
}
alias EchoAlias = Echo;
type Increment = {};
impl Increment {
    public flow run(self, value: i32) -> i32 { return value + 1; }
}
flow main() -> i32 {
    let stages = Echo | Increment | EchoAlias;
    let direct = 42 ~> Echo;
    return direct + stages(9) + stages(10);
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
    assert_eq!(result.value(), Some(&InterpValue::i32(63)));
    let instances = result
        .events
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::AgentCreated { instance, .. } => Some(*instance),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        instances.len(),
        5,
        "each Agent stage invocation constructs a fresh instance"
    );
    let requests = result
        .events
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::AgentRequestQueued { instance, .. } => Some(*instance),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|instance| instances.contains(instance)));
}

#[tokio::test(flavor = "current_thread")]
async fn autonomous_main_starts_once_after_initialization_and_retains_its_update_turn() {
    let checked = super::super::project::checked_autonomous_project();
    let entry = EntryPoint {
        item: checked.entry.unwrap(),
    };
    let args = crate::api::entry_args_from_strings(checked, Vec::new());
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let console = host.pause_console_completion();
    let events = std::sync::Arc::new(ManagedAdmissionEvents::default());
    let mut options = RunOptions::default();
    options.event_observer = Some(events.clone());
    options.host_context.authority.grants = vec![HostActionGrant::allow("Console", "stdout_write")];
    let invocation = Interpreter.create_run(checked, entry, args, &host, options);
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        let run = invocation.execute();
        tokio::pin!(run);
        let release = async {
            console.started.notified().await;
            events.wait_for_reads(1).await;
            console.release.add_permits(1);
        };
        tokio::select! {
            result = &mut run => result,
            () = release => run.await,
        }
    })
    .await
    .expect("main's pending write permits a snapshot read and resumes its update turn");
    let result = result.unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(4184)));
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(result.events.iter().filter(|event| matches!(event, WorkflowEvent::AgentBoundary { resource, .. } if resource.ends_with("Worker.main"))).count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn autonomous_main_is_cancelled_and_joined_when_its_owner_exits() {
    let checked = super::super::project::checked_autonomous_project();
    let entry = EntryPoint {
        item: checked.entry.unwrap(),
    };
    let args = crate::api::entry_args_from_strings(checked, vec!["stop".into()]);
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let console = host.pause_console_completion();
    let mut options = RunOptions::default();
    options.host_context.authority.grants = vec![
        HostActionGrant::allow("Console", "stdin_read_line"),
        HostActionGrant::allow("Console", "stdout_write"),
    ];
    let invocation = Interpreter.create_run(checked, entry, args, &host, options);
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        let run = invocation.execute();
        tokio::pin!(run);
        let release = async {
            while host.console_call_count() < 2 {
                console.started.notified().await;
            }
            // The owner is waiting on stdin. Let it leave while the automatic
            // main's stdout call is still in flight.
            console.release.add_permits(1);
            let main_signal = console.signals.lock().unwrap()[1].clone();
            main_signal.cancelled().await.unwrap();
            console.release.add_permits(1);
        };
        tokio::select! {
            result = &mut run => result,
            () = release => run.await,
        }
    })
    .await
    .expect("owner exit must cancel and join automatic work")
    .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(41)));
    assert_eq!(host.console_call_count(), 2);
    let request = result
        .events
        .iter()
        .find_map(|event| match event {
            WorkflowEvent::AgentBoundary {
                resource, request, ..
            } if resource.ends_with("Worker.main") => *request,
            _ => None,
        })
        .expect("automatic main request");
    assert!(result.events.iter().any(|event| matches!(event, WorkflowEvent::AgentRequestSettled { request: settled, committed: false, .. } if *settled == request)));
}

#[tokio::test(flavor = "current_thread")]
async fn cumulative_context_and_output_token_limits_are_independent() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.{Tokens, ContextTokens};
agent Writer(input: string) -> string ![] {
    return perform infer(Prompt.new().user(Public(input)));
}
@limits([Tokens(536870912), ContextTokens(1000000)])
flow main() -> string {
    var result = "";
    for input in ["one", "two", "three"] { result = Writer(input); }
    return result;
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    for text in ["first", "second", "third"] {
        host.seed_model_response_text_with_usage(
            text,
            Some(etas_host::ModelUsage {
                input_tokens: Some(100_000),
                output_tokens: Some(20),
                cost: None,
            }),
        );
    }
    let mut options = RunOptions::default();
    options.model_policy.options.max_output_tokens = Some(131_072);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("third".into())));
    let requests = host.model_requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request.options.max_output_tokens, Some(131_072));
        assert_eq!(request.options.max_context_tokens, Some(1_000_000));
        assert_eq!(
            request.budget.remaining_tokens().unwrap(),
            Some(536_870_912 - 300_060)
        );
    }
}
