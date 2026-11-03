use super::RunControl;
use crate::{
    api::{
        EntryPoint, InterpValue, InterpreterCheckpoint, RunInfrastructureError, RunOptions,
        RunResult,
    },
    driver::lifecycle,
    host::HostServices,
};
use etas_frontend::CheckedProject;
use etas_host::execution::{CancellationReason, ExecutionScope};

pub(crate) enum InvocationInput<'a> {
    Run {
        entry: EntryPoint,
        args: Vec<InterpValue>,
        prepared: Option<&'a std::sync::Arc<crate::api::code::PreparedCodeRevision>>,
    },
    Resume {
        checkpoint: &'a InterpreterCheckpoint,
        prepared: Option<&'a std::sync::Arc<crate::api::code::PreparedCodeRevision>>,
    },
    ResumeSource(&'a crate::api::codec::SourceCheckpoint),
}

/// Single-use execution ownership, established before any planning or polling.
pub struct RunInvocation<'a> {
    pub(crate) project: &'a CheckedProject,
    pub(crate) host: &'a dyn HostServices,
    pub(crate) options: RunOptions,
    pub(crate) input: InvocationInput<'a>,
    pub(crate) owner: InvocationOwner,
}

impl<'a> RunInvocation<'a> {
    pub(crate) fn run(
        project: &'a CheckedProject,
        entry: EntryPoint,
        args: Vec<InterpValue>,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> Self {
        Self::new(
            project,
            host,
            options,
            InvocationInput::Run {
                entry,
                args,
                prepared: None,
            },
        )
    }
    pub(crate) fn prepared(
        revision: &'a std::sync::Arc<crate::api::code::PreparedCodeRevision>,
        args: Vec<InterpValue>,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> Self {
        Self::new(
            revision.program(),
            host,
            options,
            InvocationInput::Run {
                entry: revision.entry(),
                args,
                prepared: Some(revision),
            },
        )
    }
    pub(crate) fn resume(
        project: &'a CheckedProject,
        checkpoint: &'a InterpreterCheckpoint,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> Self {
        Self::new(
            project,
            host,
            options,
            InvocationInput::Resume {
                checkpoint,
                prepared: None,
            },
        )
    }
    pub(crate) fn resume_prepared(
        revision: &'a std::sync::Arc<crate::api::code::PreparedCodeRevision>,
        checkpoint: &'a InterpreterCheckpoint,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> Self {
        Self::new(
            revision.program(),
            host,
            options,
            InvocationInput::Resume {
                checkpoint,
                prepared: Some(revision),
            },
        )
    }
    pub(crate) fn resume_source(
        project: &'a CheckedProject,
        checkpoint: &'a crate::api::codec::SourceCheckpoint,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> Self {
        Self::new(
            project,
            host,
            options,
            InvocationInput::ResumeSource(checkpoint),
        )
    }
    fn new(
        project: &'a CheckedProject,
        host: &'a dyn HostServices,
        options: RunOptions,
        input: InvocationInput<'a>,
    ) -> Self {
        Self {
            project,
            host,
            options,
            input,
            owner: InvocationOwner {
                scope: ExecutionScope::new_owned(),
                body_finished: false,
            },
        }
    }
    pub fn control(&self) -> RunControl {
        RunControl::new(self.owner.scope.clone())
    }

    pub fn execute(
        self,
    ) -> impl std::future::Future<Output = Result<RunResult, RunInfrastructureError>> + 'a {
        lifecycle::drive_invocation(self)
    }
}

pub(crate) struct InvocationOwner {
    pub scope: ExecutionScope,
    pub body_finished: bool,
}

impl Drop for InvocationOwner {
    fn drop(&mut self) {
        if !matches!(self.scope.termination(), Ok(Some(_))) {
            let _ = self
                .scope
                .cancel_source()
                .stop(CancellationReason::OwnerDropped);
            if !self.body_finished {
                let _ = self.scope.finish_body(true);
            }
        }
    }
}
