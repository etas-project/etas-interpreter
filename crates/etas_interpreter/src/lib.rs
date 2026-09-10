pub mod api;
mod control;
mod diagnostics;
mod driver;
mod eval;
pub mod host;
mod intrinsic;
mod orchestration;
mod plan;
mod value;

use api::{
    EntryPoint, InterpValue, InterpreterCheckpoint, PlanOptions, PlanResult,
    RunInfrastructureError, RunInvocation, RunOptions, RunResult,
};
use etas_frontend::CheckedProject;
use host::HostServices;

pub struct Interpreter;

impl Interpreter {
    pub fn plan(&self, project: &CheckedProject, options: PlanOptions) -> PlanResult {
        plan::build_plan(project, options)
    }

    pub fn create_run<'a>(
        &self,
        project: &'a CheckedProject,
        entry: EntryPoint,
        args: Vec<InterpValue>,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> RunInvocation<'a> {
        RunInvocation::run(project, entry, args, host, options)
    }

    pub fn create_resume<'a>(
        &self,
        project: &'a CheckedProject,
        checkpoint: &'a InterpreterCheckpoint,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> RunInvocation<'a> {
        RunInvocation::resume(project, checkpoint, host, options)
    }

    pub fn run_checked<'a>(
        &self,
        project: &'a CheckedProject,
        entry: EntryPoint,
        args: Vec<InterpValue>,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> impl std::future::Future<Output = Result<RunResult, RunInfrastructureError>> + 'a {
        self.create_run(project, entry, args, host, options)
            .execute()
    }

    pub fn resume_checkpoint<'a>(
        &self,
        project: &'a CheckedProject,
        checkpoint: &'a InterpreterCheckpoint,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> impl std::future::Future<Output = Result<RunResult, RunInfrastructureError>> + 'a {
        self.create_resume(project, checkpoint, host, options)
            .execute()
    }
}

#[cfg(test)]
mod testing;
