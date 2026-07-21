use super::*;

impl<'a> EvalContext<'a> {
    pub(crate) fn resume_perform_signal(
        &mut self,
        perform: PendingPerform,
        value: InterpValue,
    ) -> ControlSignal {
        self.resume_perform(perform, value)
    }

    pub(crate) fn resume_memory_signal(
        &mut self,
        memory: PendingMemory,
        value: InterpValue,
    ) -> ControlSignal {
        self.apply_continuation(memory.continuation, value)
    }

    pub(crate) fn resume_console_signal(
        &mut self,
        console: PendingConsole,
        value: InterpValue,
    ) -> ControlSignal {
        self.apply_continuation(console.continuation, value)
    }

    pub(crate) fn resume_command_signal(
        &mut self,
        command: PendingCommand,
        value: InterpValue,
    ) -> ControlSignal {
        self.apply_continuation(command.continuation, value)
    }

    pub(crate) fn resume_host_signal(
        &mut self,
        host: PendingHostBoundary,
        value: InterpValue,
    ) -> ControlSignal {
        self.apply_continuation(host.continuation, value)
    }
}
