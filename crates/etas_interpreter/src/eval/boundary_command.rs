use super::*;
use etas_host::CommandOutput;

impl<'a> EvalContext<'a> {
    pub(crate) fn replayed_command_result(&self, command: &PendingCommand) -> Option<InterpValue> {
        let key = self.command_boundary_key(command);
        self.completed_host_boundary_result(
            &crate::orchestration::BoundaryOccurrenceId::HostRequest(command.request.id),
            "command",
            &key,
        )
    }

    pub(crate) fn command_result_value(
        &mut self,
        command: &PendingCommand,
        result: CommandOutput,
    ) -> Option<InterpValue> {
        match command.decode {
            CommandDecode::CommandResult => Some(InterpValue::CommandResult {
                exit_code: result.exit_code,
                stdout: result.stdout,
                stderr: result.stderr,
            }),
        }
    }

    pub(crate) fn command_boundary_key(&self, command: &PendingCommand) -> String {
        format!(
            "command:argv={:?}:env={:?}:cwd={:?}:stdin={:?}",
            command.request.argv,
            command.request.env,
            command
                .request
                .cwd
                .as_ref()
                .map(|cwd| cwd.relative.display().to_string()),
            command.request.stdin
        )
    }
}
