use etas_frontend::CheckedProject;

use crate::{
    Interpreter,
    api::{
        EntryPoint, InterpValue, InterpreterCheckpoint, RunInfrastructureError, RunOptions,
        RunResult,
    },
    host::HostServices,
};

pub fn run_checked_blocking(
    project: &CheckedProject,
    entry: EntryPoint,
    args: Vec<InterpValue>,
    host: &dyn HostServices,
    options: RunOptions,
) -> Result<RunResult, RunInfrastructureError> {
    block_on_checked(Interpreter.run_checked(project, entry, args, host, options))
}

pub fn resume_checkpoint_blocking(
    project: &CheckedProject,
    checkpoint: &InterpreterCheckpoint,
    host: &dyn HostServices,
    options: RunOptions,
) -> Result<RunResult, RunInfrastructureError> {
    block_on_checked(Interpreter.resume_checkpoint(project, checkpoint, host, options))
}

fn block_on_checked(
    future: impl std::future::Future<Output = Result<RunResult, RunInfrastructureError>>,
) -> Result<RunResult, RunInfrastructureError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(RunInfrastructureError::RuntimeInitialization)?;
    runtime.block_on(future)
}
