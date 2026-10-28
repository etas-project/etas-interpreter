use super::*;

impl<'a> EvalContext<'a> {
    pub(crate) fn record_completed_host_boundary(
        &mut self,
        occurrence: crate::orchestration::BoundaryOccurrenceId,
        kind: &str,
        key: String,
        result: InterpValue,
        span: Span,
    ) -> Result<(), crate::control::ExecutionFault> {
        self.store_completed_boundary(
            crate::orchestration::CompletedHostBoundary {
                occurrence,
                kind: kind.to_owned(),
                key,
                result: crate::orchestration::CompletedHostBoundaryResult::Runtime(result),
            },
            span,
        )
    }

    pub(crate) fn record_completed_host_value_boundary(
        &mut self,
        occurrence: crate::orchestration::BoundaryOccurrenceId,
        kind: &str,
        key: String,
        result: etas_host::HostValue,
        span: Span,
    ) -> Result<(), crate::control::ExecutionFault> {
        self.store_completed_boundary(
            crate::orchestration::CompletedHostBoundary {
                occurrence,
                kind: kind.to_owned(),
                key,
                result: crate::orchestration::CompletedHostBoundaryResult::Host(result),
            },
            span,
        )
    }

    fn store_completed_boundary(
        &mut self,
        boundary: crate::orchestration::CompletedHostBoundary,
        span: Span,
    ) -> Result<(), crate::control::ExecutionFault> {
        upsert_boundary(&mut self.completed_host_boundaries, boundary).map_err(|reason| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                reason,
            )
        })
    }

    pub(crate) fn completed_host_boundary_result(
        &self,
        occurrence: &crate::orchestration::BoundaryOccurrenceId,
        kind: &str,
        key: &str,
    ) -> Option<InterpValue> {
        self.completed_host_boundaries
            .get(occurrence)
            .filter(|completed| completed.kind == kind && completed.key == key)
            .and_then(|completed| match &completed.result {
                crate::orchestration::CompletedHostBoundaryResult::Runtime(result) => {
                    Some(result.clone())
                }
                crate::orchestration::CompletedHostBoundaryResult::Host(_) => None,
            })
    }

    pub(crate) fn completed_host_boundary_host_result(
        &self,
        occurrence: &crate::orchestration::BoundaryOccurrenceId,
        kind: &str,
        key: &str,
    ) -> Option<etas_host::HostValue> {
        self.completed_host_boundaries
            .get(occurrence)
            .filter(|completed| completed.kind == kind && completed.key == key)
            .and_then(|completed| match &completed.result {
                crate::orchestration::CompletedHostBoundaryResult::Host(result) => {
                    Some(result.clone())
                }
                crate::orchestration::CompletedHostBoundaryResult::Runtime(_) => None,
            })
    }
}

fn upsert_boundary(
    ledger: &mut std::collections::BTreeMap<
        crate::orchestration::BoundaryOccurrenceId,
        crate::orchestration::CompletedHostBoundary,
    >,
    boundary: crate::orchestration::CompletedHostBoundary,
) -> Result<(), &'static str> {
    use crate::orchestration::CompletedHostBoundaryResult as R;
    if let Some(previous) = ledger.get_mut(&boundary.occurrence) {
        if previous.kind != boundary.kind || previous.key != boundary.key {
            return Err("replayed boundary occurrence differs from its recorded request identity");
        }
        if !matches!(
            (&previous.result, &boundary.result),
            (R::Runtime(_), R::Runtime(_)) | (R::Host(_), R::Host(_))
        ) {
            return Err("replayed boundary occurrence differs from its recorded result ABI");
        }
        *previous = boundary;
    } else {
        ledger.insert(boundary.occurrence.clone(), boundary);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::{
        BoundaryOccurrenceId, CompletedHostBoundary, CompletedHostBoundaryResult,
    };

    fn completion(value: i32) -> CompletedHostBoundary {
        CompletedHostBoundary {
            occurrence: BoundaryOccurrenceId::HostRequest(etas_host::HostRequestId(0)),
            kind: "memory".into(),
            key: "same-operation".into(),
            result: CompletedHostBoundaryResult::Runtime(InterpValue::i32(value)),
        }
    }

    #[test]
    fn replay_completion_updates_one_occurrence_without_changing_old_snapshots() {
        let mut ledger = std::collections::BTreeMap::new();
        upsert_boundary(&mut ledger, completion(1)).unwrap();
        let snapshot = ledger.clone();
        upsert_boundary(&mut ledger, completion(2)).unwrap();
        assert_eq!(ledger.len(), 1);
        assert!(
            matches!(&ledger.values().next().unwrap().result, CompletedHostBoundaryResult::Runtime(value) if value == &InterpValue::i32(2))
        );
        assert!(
            matches!(&snapshot.values().next().unwrap().result, CompletedHostBoundaryResult::Runtime(value) if value == &InterpValue::i32(1))
        );
    }

    #[test]
    fn replay_completion_rejects_changed_request_identity_and_result_abi() {
        for case in 0..3 {
            let mut ledger = std::collections::BTreeMap::new();
            upsert_boundary(&mut ledger, completion(1)).unwrap();
            let mut changed = completion(2);
            match case {
                0 => changed.kind = "command".into(),
                1 => changed.key = "different-operation".into(),
                2 => changed.result = CompletedHostBoundaryResult::Host(etas_host::HostValue::Unit),
                _ => unreachable!(),
            }
            assert!(upsert_boundary(&mut ledger, changed).is_err());
            assert_eq!(ledger.len(), 1);
            assert!(
                matches!(&ledger.values().next().unwrap().result, CompletedHostBoundaryResult::Runtime(value) if value == &InterpValue::i32(1))
            );
        }
    }
}
