use super::*;
use crate::control::ExecutionFault;
use etas_host::replay::ReplayFingerprint;

impl EvalContext<'_> {
    pub(crate) fn approval_request_for(
        &mut self,
        perform: &PendingPerform,
    ) -> Result<Option<(ReplayFingerprint, ApprovalRequest)>, ExecutionFault> {
        if path_name(&perform.action) != "Approval" || perform.action.action != "request" {
            return Ok(None);
        }
        let [InterpValue::String(reason)] = perform.args.as_slice() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                perform.span,
                "checked approval request requires exactly one string reason",
            ));
        };
        let request = ApprovalRequest {
            id: HostRequestId(self.next_host_request),
            reason: reason.to_string(),
            requested_grants: Vec::new(),
            trace: self.host_trace(),
        };
        let key = self.replay_fingerprint(&request, request.id, perform.span)?;
        self.next_host_request += 1;
        Ok(Some((key, request)))
    }
}
