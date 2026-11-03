use super::*;

impl<'a> EvalContext<'a> {
    pub(crate) fn record_completed_host_boundary(
        &mut self,
        occurrence: crate::orchestration::BoundaryOccurrenceId,
        kind: &str,
        key: etas_host::replay::ReplayFingerprint,
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
        key: etas_host::replay::ReplayFingerprint,
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
        key: &etas_host::replay::ReplayFingerprint,
        span: Span,
    ) -> Result<Option<InterpValue>, crate::control::ExecutionFault> {
        let Some(completed) = self.checked_completed_boundary(occurrence, kind, key, span)? else {
            return Ok(None);
        };
        match &completed.result {
            crate::orchestration::CompletedHostBoundaryResult::Runtime(result) => {
                Ok(Some(result.clone()))
            }
            _ => Err(replay_fault(
                span,
                "recorded boundary has a different result ABI",
            )),
        }
    }

    pub(crate) fn completed_host_boundary_host_result(
        &self,
        occurrence: &crate::orchestration::BoundaryOccurrenceId,
        kind: &str,
        key: &etas_host::replay::ReplayFingerprint,
        span: Span,
    ) -> Result<Option<etas_host::HostValue>, crate::control::ExecutionFault> {
        let Some(completed) = self.checked_completed_boundary(occurrence, kind, key, span)? else {
            return Ok(None);
        };
        match &completed.result {
            crate::orchestration::CompletedHostBoundaryResult::Host(result) => {
                Ok(Some(result.clone()))
            }
            _ => Err(replay_fault(
                span,
                "recorded boundary has a different result ABI",
            )),
        }
    }

    fn checked_completed_boundary(
        &self,
        occurrence: &crate::orchestration::BoundaryOccurrenceId,
        kind: &str,
        key: &etas_host::replay::ReplayFingerprint,
        span: Span,
    ) -> Result<Option<&crate::orchestration::CompletedHostBoundary>, crate::control::ExecutionFault>
    {
        let Some(completed) = self.completed_host_boundaries.get(occurrence) else {
            return Ok(None);
        };
        if completed.kind != kind || completed.key != *key {
            return Err(replay_fault(
                span,
                "recorded boundary request fingerprint does not match the current request",
            ));
        }
        Ok(Some(completed))
    }
}

fn replay_fault(span: Span, message: &str) -> crate::control::ExecutionFault {
    crate::control::ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
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
            key: etas_host::replay::ReplayFingerprint::from_bytes([1; 32]),
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
                1 => changed.key = etas_host::replay::ReplayFingerprint::from_bytes([2; 32]),
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

    #[test]
    fn replay_lookup_rejects_mismatch_instead_of_turning_it_into_a_cache_miss() {
        let checked = crate::testing::project::checked_project(
            "module app.main; flow main() -> unit { return; }",
        );
        let plan = crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
            .unwrap();
        let (program, mut state) = EvalContextInput {
            replay_keys: super::super::replay::RunReplayKeys::start(
                &crate::api::RunOptions::default(),
            )
            .unwrap(),
            code_definitions: None,
            syntax_limits: Default::default(),
            storage_limits: Default::default(),
            event_observer: None,
            execution: etas_host::execution::ExecutionScope::new_owned(),
            checked: &checked,
            plan: &plan,
            host_context: Default::default(),
            model_policy: Default::default(),
            execution_limits: Default::default(),
            consumed_steps: 0,
            current_session: None,
            entry_item: checked.entry.unwrap(),
            entry_args: &[],
        }
        .into_parts();
        let original = completion(1);
        state
            .completed_host_boundaries
            .insert(original.occurrence.clone(), original.clone());
        let ctx = EvalContext::new(program, &mut state);
        let span = item_span(&checked, checked.entry.unwrap());
        assert_eq!(
            ctx.completed_host_boundary_result(&original.occurrence, "memory", &original.key, span)
                .unwrap(),
            Some(InterpValue::i32(1))
        );
        assert!(
            ctx.completed_host_boundary_result(
                &original.occurrence,
                "approval",
                &original.key,
                span
            )
            .is_err()
        );
        assert!(
            ctx.completed_host_boundary_result(
                &original.occurrence,
                "memory",
                &etas_host::replay::ReplayFingerprint::from_bytes([2; 32]),
                span
            )
            .is_err()
        );
        assert!(
            ctx.completed_host_boundary_host_result(
                &original.occurrence,
                "memory",
                &original.key,
                span
            )
            .is_err()
        );
        assert!(
            ctx.completed_host_boundary_result(
                &BoundaryOccurrenceId::HostRequest(etas_host::HostRequestId(1)),
                "memory",
                &original.key,
                span
            )
            .unwrap()
            .is_none()
        );
    }
}
