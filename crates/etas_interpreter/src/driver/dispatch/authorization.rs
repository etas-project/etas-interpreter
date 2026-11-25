use super::{
    host_dispatch::{HostCompletion, HostDispatch},
    policy::AuthorizationRequest,
};
use crate::{
    control::{
        AuthorizedNativeBoundary, ControlSignal, NativeBoundaryTarget, PendingNativeAuthorization,
        PendingTaskControl,
    },
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
};
use etas_host::{HostError, HostTraceRequest, HostValue, PolicySubject};
use std::{future::Future, pin::Pin, rc::Rc};

pub(in crate::driver) enum AuthorizationStart<'host> {
    Complete(ControlSignal),
    Pending(Box<PendingAuthorizationDispatch<'host>>),
}
pub(in crate::driver) struct PendingAuthorizationDispatch<'host> {
    state: PendingNativeAuthorization,
    operation: Rc<crate::driver::scheduler::HostOperation>,
    future: Pin<Box<dyn Future<Output = AuthorizationCompletion> + 'host>>,
}
pub(in crate::driver) struct CompletedAuthorizationDispatch {
    state: PendingNativeAuthorization,
    operation: Rc<crate::driver::scheduler::HostOperation>,
    completion: AuthorizationCompletion,
}
enum AuthorizationCompletion {
    Policy(HostCompletion<etas_host::PolicyResponse>),
    Approval(HostCompletion<etas_host::ApprovalResponse>),
}
impl PendingAuthorizationDispatch<'_> {
    pub(in crate::driver) fn operation_state(&self) -> Rc<crate::driver::scheduler::HostOperation> {
        self.operation.clone()
    }
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.operation.id()
    }
    pub(in crate::driver) async fn poll(self) -> CompletedAuthorizationDispatch {
        CompletedAuthorizationDispatch {
            state: self.state,
            operation: self.operation,
            completion: self.future.await,
        }
    }
}

pub(in crate::driver) fn prepare(
    eval: &EvalContext<'_>,
    target: NativeBoundaryTarget,
    checks: Vec<(Option<HostValue>, PolicySubject)>,
    budget: etas_host::ExecutionBudget,
) -> ControlSignal {
    let checks = checks
        .into_iter()
        .map(|(reference, subject)| {
            crate::control::PolicyAuthorization::new(eval, reference, subject)
        })
        .collect();
    ControlSignal::pending_task(PendingTaskControl::NativeAuthorization(Box::new(
        PendingNativeAuthorization {
            target,
            checks,
            budget,
            context: eval.task.capture_agent_host_context(),
        },
    )))
}

pub(in crate::driver) fn advance<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    mut state: PendingNativeAuthorization,
) -> AuthorizationStart<'host> {
    let lease = state
        .context
        .clone()
        .map(|context| eval.task.borrow_agent_host_context(context, None));
    let next = loop {
        if let Err(error) = state.budget.check_time() {
            return AuthorizationStart::Complete(failure(eval, state.target, error));
        }
        let Some(check) = state.checks.front_mut() else {
            break None;
        };
        match check.next_request(eval) {
            Ok(Some(request)) => break Some(request),
            Ok(None) => {
                state.checks.pop_front();
            }
            Err(error) => return AuthorizationStart::Complete(failure(eval, state.target, error)),
        }
    };
    let started = match next {
        None => {
            drop(lease);
            return AuthorizationStart::Complete(ControlSignal::pending_task(
                PendingTaskControl::AuthorizedNative(Box::new(AuthorizedNativeBoundary {
                    target: state.target,
                    context: state.context,
                })),
            ));
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
    let (operation, future) = match started {
        Ok(started) => started,
        Err(error) => return AuthorizationStart::Complete(failure(eval, state.target, error)),
    };
    drop(lease);
    AuthorizationStart::Pending(Box::new(PendingAuthorizationDispatch {
        state,
        operation,
        future,
    }))
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    completed: CompletedAuthorizationDispatch,
) -> ControlSignal {
    let CompletedAuthorizationDispatch {
        mut state,
        operation,
        completion,
    } = completed;
    // Native evidence settles even when the language continuation was abandoned.
    let response = match completion {
        AuthorizationCompletion::Policy(completion) => {
            HostDispatch::finish(eval, completion).map(|value| (Some(value), None))
        }
        AuthorizationCompletion::Approval(completion) => {
            HostDispatch::finish(eval, completion).map(|value| (None, Some(value)))
        }
    };
    if !operation.is_waiting() {
        return ControlSignal::runtime_fault(
            "native authorization continuation was abandoned",
            state.target.span(),
        );
    }
    let accepted = response.and_then(|(policy, approval)| {
        state.budget.check_time()?;
        let check = state
            .checks
            .front_mut()
            .expect("pending authorization check");
        match (policy, approval) {
            (Some(response), None) => check.accept_policy(response),
            (None, Some(response)) => check.accept_approval(eval, response),
            _ => unreachable!("one authorization response"),
        }
    });
    if let Err(error) = accepted {
        return failure(eval, state.target, error);
    }
    ControlSignal::pending_task(PendingTaskControl::NativeAuthorization(Box::new(state)))
}

fn failure(
    eval: &mut EvalContext<'_>,
    target: NativeBoundaryTarget,
    error: HostError,
) -> ControlSignal {
    match eval
        .task
        .host_execution()
        .signal()
        .and_then(|signal| signal.cause())
    {
        Ok(Some(cause)) => return ControlSignal::Cancelled(cause),
        Err(error) => return ControlSignal::missing_checked_fact(error.to_string(), target.span()),
        Ok(None) => {}
    }
    let span = target.span();
    let kind = match target {
        NativeBoundaryTarget::Host { pending, key } => {
            return super::host::authorization_failure(eval, pending, key, error);
        }
        NativeBoundaryTarget::Session { pending, .. } => {
            return eval.session_host_error_signal(pending, error);
        }
        NativeBoundaryTarget::Compiler { perform, .. } => {
            return super::compiler::compiler_error(eval, perform, error);
        }
        NativeBoundaryTarget::Console { .. } => "console",
        NativeBoundaryTarget::Command { .. } => "command",
        NativeBoundaryTarget::Memory { .. } => "memory",
        NativeBoundaryTarget::Model(_) => "model",
        NativeBoundaryTarget::Tool(_) => "tool",
    };
    ControlSignal::runtime_fault(super::policy::policy_failure_message(kind, &error), span)
}

pub(in crate::driver) enum AuthorizedDispatchStart<'host> {
    Signal(ControlSignal),
    Ready,
    Failed,
    Pending(AuthorizedHostDispatch<'host>),
}
pub(in crate::driver) enum AuthorizedHostDispatch<'host> {
    Console(Box<super::console::PendingConsoleDispatch<'host>>),
    Command(Box<super::command::PendingCommandDispatch<'host>>),
    Memory(Box<super::memory::PendingMemoryDispatch<'host>>),
    Host(Box<super::host::PendingHostBoundaryDispatch<'host>>),
    Model(Box<super::model::PendingModelHostDispatch<'host>>),
    Tool(Box<super::tool::PendingToolHostDispatch<'host>>),
    Compiler(Box<super::compiler::PendingCompilerDispatch<'host>>),
}

pub(in crate::driver) async fn dispatch<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    machine: &mut EvalMachine,
    approved: AuthorizedNativeBoundary,
) -> AuthorizedDispatchStart<'host> {
    let _lease = approved
        .context
        .map(|context| eval.task.borrow_agent_host_context(context, None));
    use AuthorizedDispatchStart as Start;
    use AuthorizedHostDispatch as Pending;
    match approved.target {
        NativeBoundaryTarget::Console { pending, key } => {
            match super::console::begin_authorized(eval, host, pending, key).await {
                super::console::ConsoleDispatchStart::Complete(Some(signal)) => {
                    Start::Signal(signal)
                }
                super::console::ConsoleDispatchStart::Complete(None) => Start::Failed,
                super::console::ConsoleDispatchStart::Pending(pending) => {
                    Start::Pending(Pending::Console(pending))
                }
            }
        }
        NativeBoundaryTarget::Command { pending, key } => {
            match super::command::begin_authorized(eval, host, pending, key, machine).await {
                super::command::CommandDispatchStart::Complete(Some(signal)) => {
                    Start::Signal(signal)
                }
                super::command::CommandDispatchStart::Complete(None) => Start::Failed,
                super::command::CommandDispatchStart::Pending(pending) => {
                    Start::Pending(Pending::Command(pending))
                }
            }
        }
        NativeBoundaryTarget::Memory { pending, key } => {
            match super::memory::begin_authorized(eval, host, pending, key, machine).await {
                super::memory::MemoryDispatchStart::Complete(Some(signal)) => Start::Signal(signal),
                super::memory::MemoryDispatchStart::Complete(None) => Start::Failed,
                super::memory::MemoryDispatchStart::Pending(pending) => {
                    Start::Pending(Pending::Memory(pending))
                }
            }
        }
        NativeBoundaryTarget::Host { pending, key } => {
            match super::host::begin_authorized(eval, host, pending, key, machine).await {
                super::host::HostBoundaryDispatchStart::Complete(Some(signal)) => {
                    Start::Signal(signal)
                }
                super::host::HostBoundaryDispatchStart::Complete(None) => Start::Failed,
                super::host::HostBoundaryDispatchStart::Pending(pending) => {
                    Start::Pending(Pending::Host(pending))
                }
            }
        }
        NativeBoundaryTarget::Model(pending) => {
            match super::model::begin_authorized(eval, host, pending, machine).await {
                super::model::ModelDispatchStart::Complete(true) => Start::Ready,
                super::model::ModelDispatchStart::Complete(false) => Start::Failed,
                super::model::ModelDispatchStart::Pending(pending) => {
                    Start::Pending(Pending::Model(pending))
                }
            }
        }
        NativeBoundaryTarget::Tool(pending) => {
            match super::tool::begin_authorized(eval, host, pending, machine).await {
                super::tool::ToolDispatchStart::Complete(true) => Start::Ready,
                super::tool::ToolDispatchStart::Complete(false) => Start::Failed,
                super::tool::ToolDispatchStart::Pending(pending) => {
                    Start::Pending(Pending::Tool(pending))
                }
            }
        }
        NativeBoundaryTarget::Session { pending, key } => {
            Start::Signal(super::session::dispatch_authorized(eval, host, pending, key).await)
        }
        NativeBoundaryTarget::Compiler {
            invocation,
            perform,
        } => match super::compiler::begin_authorized(eval, host, invocation, perform).await {
            super::compiler::CompilerDispatchStart::Complete(signal) => Start::Signal(signal),
            super::compiler::CompilerDispatchStart::Pending(pending) => {
                Start::Pending(Pending::Compiler(pending))
            }
            super::compiler::CompilerDispatchStart::NotCompiler(_) => {
                unreachable!("checked compiler admission")
            }
        },
    }
}
