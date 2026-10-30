use super::*;
use etas_host::CommandOutput;

impl<'a> EvalContext<'a> {
    pub(crate) fn command_result_value(
        &mut self,
        command: &PendingCommand,
        result: CommandOutput,
    ) -> Option<InterpValue> {
        match command.decode {
            CommandDecode::CommandResult => Some(InterpValue::CommandResult {
                exit_code: result.exit_code,
                stdout: result.stdout.into(),
                stderr: result.stderr.into(),
            }),
        }
    }

    pub(crate) fn command_boundary_key(
        &self,
        command: &PendingCommand,
    ) -> Result<etas_host::replay::ReplayFingerprint, crate::control::ExecutionFault> {
        self.replay_fingerprint(&command.request, command.request.id, command.span)
    }
}
