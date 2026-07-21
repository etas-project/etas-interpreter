use super::host_value::boundary_key_fragment;
use super::*;

impl<'a> EvalContext<'a> {
    pub(crate) fn replayed_approval_result(&self, perform: &PendingPerform) -> Option<InterpValue> {
        let key = self.approval_boundary_key(perform)?;
        self.completed_host_boundary_result("approval", &key)
    }

    pub(crate) fn approval_request_for(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<(String, ApprovalRequest)> {
        let key = self.approval_boundary_key(perform)?;
        let reason = perform
            .args
            .first()
            .and_then(|value| match value {
                InterpValue::String(text) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "approval".to_owned());
        let id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        Some(ApprovalRequest {
            id,
            reason,
            requested_grants: Vec::new(),
            trace: self.host_trace(),
        })
        .map(|request| (key, request))
    }

    fn approval_boundary_key(&self, perform: &PendingPerform) -> Option<String> {
        if path_name(&perform.action) != "Approval" || perform.action.action != "request" {
            return None;
        }
        let args = perform
            .args
            .iter()
            .map(boundary_key_fragment)
            .collect::<Vec<_>>()
            .join("|");
        Some(format!(
            "approval:{}:{}:{}",
            perform
                .expr
                .map(|expr| expr.0.to_string())
                .unwrap_or_else(|| "host".to_owned()),
            path_name(&perform.action),
            args
        ))
    }
}
