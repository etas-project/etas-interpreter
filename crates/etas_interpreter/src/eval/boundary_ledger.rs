use super::*;

impl<'a> EvalContext<'a> {
    pub(crate) fn record_completed_host_boundary(
        &mut self,
        kind: &str,
        key: String,
        result: InterpValue,
    ) {
        self.completed_host_boundaries
            .push(crate::orchestration::CompletedHostBoundary {
                kind: kind.to_owned(),
                key,
                result: crate::orchestration::CompletedHostBoundaryResult::Runtime(result),
            });
    }

    pub(crate) fn record_completed_host_value_boundary(
        &mut self,
        kind: &str,
        key: String,
        result: etas_host::HostValue,
    ) {
        self.completed_host_boundaries
            .push(crate::orchestration::CompletedHostBoundary {
                kind: kind.to_owned(),
                key,
                result: crate::orchestration::CompletedHostBoundaryResult::Host(result),
            });
    }

    pub(crate) fn completed_host_boundary_result(
        &self,
        kind: &str,
        key: &str,
    ) -> Option<InterpValue> {
        self.completed_host_boundaries
            .iter()
            .rev()
            .find(|completed| completed.kind == kind && completed.key == key)
            .and_then(|completed| match &completed.result {
                crate::orchestration::CompletedHostBoundaryResult::Runtime(result) => {
                    Some(result.clone())
                }
                crate::orchestration::CompletedHostBoundaryResult::Host(_) => None,
            })
    }

    pub(crate) fn completed_host_boundary_host_result(
        &self,
        kind: &str,
        key: &str,
    ) -> Option<etas_host::HostValue> {
        self.completed_host_boundaries
            .iter()
            .rev()
            .find(|completed| completed.kind == kind && completed.key == key)
            .and_then(|completed| match &completed.result {
                crate::orchestration::CompletedHostBoundaryResult::Host(result) => {
                    Some(result.clone())
                }
                crate::orchestration::CompletedHostBoundaryResult::Runtime(_) => None,
            })
    }
}
