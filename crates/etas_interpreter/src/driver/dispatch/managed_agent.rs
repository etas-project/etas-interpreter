use super::{
    host_dispatch::{HostCompletion, HostDispatch},
    policy::AuthorizationRequest,
};
use crate::{
    control::{
        ControlSignal, PendingManagedAgent, PendingManagedAuthorization, PendingTaskControl,
        managed_agent::{ManagedAgentOperation, ManagedAgentOrigin},
    },
    eval::EvalContext,
    host::HostServices,
    orchestration::WorkflowEvent,
};
use etas_host::{HostError, HostTraceRequest, HostValue, PolicySubject};
use std::{future::Future, pin::Pin, rc::Rc};

pub(in crate::driver) enum ManagedDispatchStart<'host> {
    Complete(ControlSignal),
    Pending(Box<PendingManagedAuthorizationDispatch<'host>>),
}

pub(in crate::driver) struct PendingManagedAuthorizationDispatch<'host> {
    state: PendingManagedAuthorization,
    operation: Rc<crate::driver::scheduler::HostOperation>,
    future: Pin<Box<dyn Future<Output = AuthorizationCompletion> + 'host>>,
}

pub(in crate::driver) struct CompletedManagedAuthorizationDispatch {
    state: PendingManagedAuthorization,
    operation: Rc<crate::driver::scheduler::HostOperation>,
    completion: AuthorizationCompletion,
}

enum AuthorizationCompletion {
    Policy(HostCompletion<etas_host::PolicyResponse>),
    Approval(HostCompletion<etas_host::ApprovalResponse>),
}

impl<'host> PendingManagedAuthorizationDispatch<'host> {
    pub(in crate::driver) fn operation_state(&self) -> Rc<crate::driver::scheduler::HostOperation> {
        self.operation.clone()
    }
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.operation.id()
    }
    pub(in crate::driver) async fn poll(self) -> CompletedManagedAuthorizationDispatch {
        CompletedManagedAuthorizationDispatch {
            state: self.state,
            operation: self.operation,
            completion: self.future.await,
        }
    }
}

fn rejected(eval: &EvalContext<'_>, span: etas_core::Span, error: HostError) -> ControlSignal {
    match eval
        .task
        .host_execution()
        .signal()
        .and_then(|signal| signal.cause())
    {
        Ok(Some(cause)) => ControlSignal::Cancelled(cause),
        Ok(None) => ControlSignal::runtime_fault(
            super::policy::policy_failure_message("agent", &error),
            span,
        ),
        Err(error) => ControlSignal::missing_checked_fact(error.to_string(), span),
    }
}

fn cancel_unadmitted(
    eval: &mut EvalContext<'_>,
    request: Option<crate::driver::scheduler::TaskHandle>,
) {
    if let Some(request) = request {
        eval.scheduler
            .cancel(
                request,
                etas_host::execution::CancellationReason::OwnerDropped,
            )
            .expect("unadmitted request cleanup");
    }
}

pub(in crate::driver) fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    pending: PendingManagedAgent,
) -> ManagedDispatchStart<'host> {
    if let Err(reason) = pending.proof.validate_origin() {
        return ManagedDispatchStart::Complete(ControlSignal::missing_checked_fact(
            reason,
            pending.span,
        ));
    }
    let (component, method) = match &pending.origin {
        ManagedAgentOrigin::Create { .. } => (pending.proof.output, None),
        ManagedAgentOrigin::Call { item, receiver, .. } => {
            let etas_hir::HirItem::Flow(flow) = &eval.checked.hir.items[*item] else {
                unreachable!("validated managed method")
            };
            (*receiver, Some(flow.symbol))
        }
    };
    let component = etas_types::ty::display_type(&eval.checked.type_store, component);
    let resource = method.map_or_else(
        || component.clone(),
        |method| {
            format!(
                "{component}.{}",
                eval.checked
                    .types
                    .agent_methods
                    .get(&method)
                    .expect("checked Agent method")
                    .name
            )
        },
    );
    let action = format!("Agentic.{}", pending.origin.name());
    let parent_request = match eval.task.agent_request_root() {
        Some(crate::driver::scheduler::TaskIdentity::Scheduled(task)) => Some(task.0),
        _ => None,
    };
    let revision = eval
        .revision
        .map(|revision| revision.snapshot_fingerprint().to_string());
    let mut subject = PolicySubject {
        kind: "agentic".into(),
        attributes: vec![
            ("action_kind".into(), HostValue::String("agentic".into())),
            ("qualified_action".into(), HostValue::String(action.clone())),
            ("resource".into(), HostValue::String(resource.clone())),
            ("agent".into(), HostValue::String(component)),
            (
                "source_id".into(),
                HostValue::UInt(pending.span.source.0.into()),
            ),
        ],
    };
    if let Some(method) = method {
        subject
            .attributes
            .push(("method_symbol".into(), HostValue::UInt(method.0.into())));
    }
    if let Some(revision) = &revision {
        subject
            .attributes
            .push(("revision".into(), HostValue::String(revision.clone())));
    }
    if let Some(parent) = parent_request {
        subject
            .attributes
            .push(("parent_request".into(), HostValue::UInt(parent.into())));
    }
    let span = pending.span;
    let (request, context, budget, authorization) = match pending.operation.clone() {
        ManagedAgentOperation::Initialize { .. } => {
            let budget = match eval.task.model_policy.budget.clone() {
                Some(limits) => match eval.host_budget().with_limits(limits) {
                    Ok(budget) => budget,
                    Err(error) => {
                        return ManagedDispatchStart::Complete(rejected(eval, span, error));
                    }
                },
                None => eval.host_budget(),
            };
            (
                None,
                eval.task.capture_agent_host_context(),
                budget,
                crate::control::PolicyAuthorization::new(eval, eval.boundary_policy_ref(), subject),
            )
        }
        ManagedAgentOperation::Submit {
            reference,
            target,
            args,
            ..
        } => {
            let program = eval.checked_execution_program();
            let handle = match eval.prepare_agent_request(reference, target, args, span) {
                Ok(handle) => handle,
                Err(error) => {
                    return ManagedDispatchStart::Complete(ControlSignal::runtime_fault(
                        error.to_string(),
                        span,
                    ));
                }
            };
            subject
                .attributes
                .push(("request".into(), HostValue::UInt(handle.task.0.into())));
            subject
                .attributes
                .push(("instance".into(), HostValue::UInt(reference.id.into())));
            let mut task = eval.scheduler.take_ready_task(handle);
            let request = eval.scheduled_task_context(
                &program,
                &mut task.local,
                handle.task,
                eval.handler_floor,
            );
            let context = request.task.capture_agent_host_context();
            let budget = request.host_budget();
            let authorization = crate::control::PolicyAuthorization::new(
                &request,
                request.boundary_policy_ref(),
                subject,
            );
            drop(request);
            eval.scheduler.restore_ready_task(handle, task);
            eval.scheduler
                .hold_agent_request(handle, reference)
                .expect("new bound request is ready");
            (Some(handle), context, budget, authorization)
        }
    };
    let event = WorkflowEvent::AgentBoundary {
        action,
        resource,
        request: request.map(|handle| handle.task.0),
        parent_request,
        instance: match &pending.operation {
            ManagedAgentOperation::Submit { reference, .. } => Some(reference.id),
            _ => None,
        },
        method: method.map(|method| method.0),
        source: span.source.0,
        revision,
    };
    advance(
        eval,
        host,
        PendingManagedAuthorization {
            original: pending,
            authorization,
            context,
            budget,
            request,
            event,
        },
    )
}

pub(in crate::driver) fn advance<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    mut state: PendingManagedAuthorization,
) -> ManagedDispatchStart<'host> {
    let lease = state
        .context
        .clone()
        .map(|context| eval.task.borrow_agent_host_context(context, None));
    let next = state
        .budget
        .check_time()
        .and_then(|()| state.authorization.next_request(eval));
    let next = match next {
        Ok(next) => next,
        Err(error) => {
            let signal = rejected(eval, state.original.span, error);
            drop(lease);
            cancel_unadmitted(eval, state.request);
            return ManagedDispatchStart::Complete(signal);
        }
    };
    let started = match next {
        None => {
            drop(lease);
            return ManagedDispatchStart::Complete(admit(eval, state));
        }
        Some(AuthorizationRequest::Policy(request)) => HostDispatch::begin(
            eval,
            request.id,
            etas_host::HostRequestKind::Policy,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| host.policy(operation, request),
        )
        .map(|pending| {
            let operation = pending.operation_state();
            let future = Box::pin(async move {
                AuthorizationCompletion::Policy(HostDispatch::poll(pending).await)
            })
                as Pin<Box<dyn Future<Output = AuthorizationCompletion> + 'host>>;
            (operation, future)
        }),
        Some(AuthorizationRequest::Approval { request, authority }) => {
            HostDispatch::begin_approval(eval, request.clone(), authority, |operation| {
                host.approval(operation, request)
            })
            .map(|pending| {
                let operation = pending.operation_state();
                let future = Box::pin(async move {
                    AuthorizationCompletion::Approval(HostDispatch::poll(pending).await)
                })
                    as Pin<Box<dyn Future<Output = AuthorizationCompletion> + 'host>>;
                (operation, future)
            })
        }
    };
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            let signal = rejected(eval, state.original.span, error);
            drop(lease);
            cancel_unadmitted(eval, state.request);
            return ManagedDispatchStart::Complete(signal);
        }
    };
    drop(lease);
    ManagedDispatchStart::Pending(Box::new(PendingManagedAuthorizationDispatch {
        state,
        operation: started.0,
        future: started.1,
    }))
}

fn admit(eval: &mut EvalContext<'_>, state: PendingManagedAuthorization) -> ControlSignal {
    if let Some(request) = state.request {
        let ManagedAgentOperation::Submit {
            reference,
            updating,
            autonomous,
            args,
            ..
        } = &state.original.operation
        else {
            unreachable!("request admission")
        };
        if let Err(error) = eval.scheduler.admit_agent_request(
            request,
            *reference,
            *updating,
            &eval.checked_execution_program(),
        ) {
            cancel_unadmitted(eval, Some(request));
            return ControlSignal::runtime_fault(error.to_string(), state.original.span);
        }
        eval.scheduler
            .record_handoff_deliveries(args, request.task.0);
        eval.events.push(state.event);
        for event in eval.scheduler.take_agent_events() {
            eval.events.push(event);
        }
        if *autonomous {
            let ManagedAgentOperation::Submit { target, args, .. } = &state.original.operation
            else {
                unreachable!()
            };
            eval.scheduler
                .mark_autonomous_main(
                    request,
                    target.clone(),
                    args.clone(),
                    state.original.span,
                    *state
                        .original
                        .autonomous_completion
                        .expect("automatic call owns its result continuation"),
                )
                .expect("admitted autonomous request");
            eval.continue_chain_signal(
                ControlSignal::Value(crate::value::InterpValue::Agent(*reference)),
                state.original.continuation,
            )
        } else {
            ControlSignal::pending_task(PendingTaskControl::AgentReply {
                task: request,
                continuation: state.original.continuation,
                span: state.original.span,
            })
        }
    } else {
        eval.events.push(state.event);
        eval.complete_managed_agent(state.original)
    }
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    completed: CompletedManagedAuthorizationDispatch,
) -> ControlSignal {
    let CompletedManagedAuthorizationDispatch {
        mut state,
        operation,
        completion,
    } = completed;
    let result = match completion {
        AuthorizationCompletion::Policy(completion) => {
            HostDispatch::finish(eval, completion).map(|response| (Some(response), None))
        }
        AuthorizationCompletion::Approval(completion) => {
            HostDispatch::finish(eval, completion).map(|response| (None, Some(response)))
        }
    };
    if !operation.is_waiting() {
        cancel_unadmitted(eval, state.request);
        return ControlSignal::runtime_fault(
            "Agent admission continuation was abandoned",
            state.original.span,
        );
    }
    let result = result.and_then(|(policy, approval)| {
        state.budget.check_time()?;
        match (policy, approval) {
            (Some(response), None) => state.authorization.accept_policy(response),
            (None, Some(response)) => state.authorization.accept_approval(eval, response),
            _ => unreachable!("one managed authorization response"),
        }
    });
    if let Err(error) = result {
        let signal = rejected(eval, state.original.span, error);
        cancel_unadmitted(eval, state.request);
        return signal;
    }
    ControlSignal::pending_task(PendingTaskControl::ManagedAuthorization(Box::new(state)))
}
