use etas_frontend::CheckedProject;

use crate::{
    Interpreter,
    api::{EntryPoint, InterpValue, InterpreterCheckpoint, RunOptions, RunResult},
    host::HostServices,
};

pub fn run_checked_blocking(
    project: &CheckedProject,
    entry: EntryPoint,
    args: Vec<InterpValue>,
    host: &dyn HostServices,
    options: RunOptions,
) -> RunResult {
    block_on_checked(Interpreter.run_checked(project, entry, args, host, options))
}

pub fn resume_checkpoint_blocking(
    project: &CheckedProject,
    checkpoint: &InterpreterCheckpoint,
    host: &dyn HostServices,
    options: RunOptions,
) -> RunResult {
    block_on_checked(Interpreter.resume_checkpoint(project, checkpoint, host, options))
}

fn block_on_checked(future: impl std::future::Future<Output = RunResult>) -> RunResult {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(future),
        Err(error) => RunResult {
            value: None,
            diagnostics: vec![etas_core::Diagnostic::analysis(
                etas_core::AnalysisDiagnosticCode::UnhandledRuntimeError,
                etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO),
                format!("failed to initialize async runtime: {error}"),
            )],
            events: Vec::new(),
            checkpoints: Vec::new(),
        },
    }
}
