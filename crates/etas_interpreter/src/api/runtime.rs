use std::{num::NonZeroUsize, sync::Arc};

use crate::driver::cpu::{CpuExecutor, SharedCpuPool};

/// A host-owned, finite CPU worker pool shared by multiple invocations.
///
/// Clone this handle into `RunOptions::runtime`. Each invocation has its own
/// `cpu_threads` quota, budget, completions and `RunControl` cancellation scope.
/// Without a runtime handle, invocations create their own isolated executor.
///
/// ```no_run
/// use std::num::NonZeroUsize;
/// use etas_interpreter::{Interpreter, api::{EntryPoint, InterpreterRuntime, RunOptions}};
/// # async fn run_two(project: &etas_frontend::CheckedProject,
/// # host: &dyn etas_interpreter::host::HostServices) -> Result<(), Box<dyn std::error::Error>> {
/// let runtime = InterpreterRuntime::new(NonZeroUsize::new(4).unwrap())?;
/// let options = |quota| RunOptions {
///     runtime: Some(runtime.clone()),
///     cpu_threads: NonZeroUsize::new(quota).unwrap(),
///     ..RunOptions::default()
/// };
/// let entry = EntryPoint { item: project.entry.unwrap() };
/// let first = Interpreter.create_run(project, entry, vec![], host, options(1));
/// let second = Interpreter.create_run(project, entry, vec![], host, options(3));
/// let (first, second) = tokio::join!(first.execute(), second.execute());
/// first?;
/// second?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct InterpreterRuntime {
    pool: Arc<SharedCpuPool>,
}

impl InterpreterRuntime {
    pub fn new(cpu_threads: NonZeroUsize) -> Result<Self, std::io::Error> {
        Ok(Self {
            pool: Arc::new(SharedCpuPool::new(cpu_threads)?),
        })
    }

    pub fn cpu_threads(&self) -> NonZeroUsize {
        self.pool.capacity()
    }

    pub(crate) fn executor(&self, quota: NonZeroUsize) -> Arc<dyn CpuExecutor> {
        Arc::new(self.pool.executor(quota))
    }
}

impl std::fmt::Debug for InterpreterRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterpreterRuntime")
            .field("cpu_threads", &self.cpu_threads())
            .finish()
    }
}
