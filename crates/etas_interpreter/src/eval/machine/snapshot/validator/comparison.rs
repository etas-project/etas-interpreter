use super::*;
use etas_frontend::CheckedCallableRoot;

impl SnapshotValidator<'_> {
    pub(super) fn collection_comparator(
        &self,
        target: &CallTargetSnapshot,
        scope: call_target::ReferenceScope,
    ) -> Result<(), String> {
        self.call_target_in(target, "checkpoint priority comparison", scope)?;
        let mut pending = vec![(target, None::<crate::value::CheckedExecutionProgram>)];
        let mut visited = std::collections::HashSet::new();
        while let Some((target, owner)) = pending.pop() {
            let checked = owner.as_ref().map_or(self.checked, |owner| &owner.checked);
            let dispatch = owner
                .as_ref()
                .map_or(self.dispatch, |owner| &owner.plan.dispatch);
            let root = match target {
                CallTargetSnapshot::FlowItem(item) | CallTargetSnapshot::ToolItem(item) => {
                    CheckedCallableRoot::Item(*item)
                }
                CallTargetSnapshot::Lambda { expr, .. } => CheckedCallableRoot::Lambda(*expr),
                CallTargetSnapshot::PureIntrinsic {
                    intrinsic,
                    parameter_types,
                    result_type,
                } => {
                    dispatch.intrinsic_reference_root(*intrinsic, parameter_types, *result_type)?
                }
                CallTargetSnapshot::StdIntrinsic { origin, .. } => {
                    CheckedCallableRoot::StdReference(*origin)
                }
                CallTargetSnapshot::Specialized { target, .. }
                | CallTargetSnapshot::Limited { target, .. } => {
                    if target.shared_identity().is_none_or(|key| {
                        visited.insert((std::sync::Arc::as_ptr(checked) as usize, 0, key as usize))
                    }) {
                        pending.push((target, owner));
                    }
                    continue;
                }
                CallTargetSnapshot::Composed(children) => {
                    if children.shared_identity().is_some_and(|key| {
                        !visited.insert((std::sync::Arc::as_ptr(checked) as usize, 1, key as usize))
                    }) {
                        continue;
                    }
                    pending.extend(children.iter().map(|child| (child, owner.clone())));
                    continue;
                }
                CallTargetSnapshot::Scoped { metadata, target } => {
                    let (program, _) = if let Some(owner) = &owner {
                        SnapshotValidator::new(
                            &owner.checked,
                            &owner.plan.slots,
                            &owner.plan.dispatch,
                            &owner.plan.closures,
                            self.limits,
                        )
                        .with_revisions(self.revisions)
                        .scoped_callable(metadata, target)?
                    } else {
                        self.scoped_callable(metadata, target)?
                    };
                    pending.push((target, Some(program)));
                    continue;
                }
                _ => {
                    return Err(
                        "checkpoint priority comparison requires a checked pure flow".into(),
                    );
                }
            };
            let summary = checked
                .checked_callable_effects(root)
                .map_err(|error| error.to_string())?;
            crate::eval::collection_ordering::validate_pure_summary(summary)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_comparator_snapshot_validation_does_not_expand_shared_dags() {
        let checked = crate::testing::project::checked_project(
            "module app.main; flow identity(value: i32) -> i32 ![] { return value; } flow main() -> unit { return; }",
        );
        let plan = crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
            .unwrap();
        let item = checked
            .hir
            .items
            .iter()
            .find_map(|(item, decl)| match decl {
                etas_hir::HirItem::Flow(flow)
                    if checked.symbols.get(flow.symbol).unwrap().name == "identity" =>
                {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        let mut target = CallTargetSnapshot::FlowItem(item);
        for _ in 0..4000 {
            let link: crate::orchestration::CallTargetSnapshotLink = target.into();
            target = CallTargetSnapshot::Composed(
                vec![
                    CallTargetSnapshot::Limited {
                        target: link.clone(),
                        limits: vec![],
                    },
                    CallTargetSnapshot::Limited {
                        target: link,
                        limits: vec![],
                    },
                ]
                .into(),
            );
        }
        let limits = etas_host::StorageLimits::default();
        let validator = SnapshotValidator::new(
            &checked,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &limits,
        );
        validator
            .collection_comparator(&target, call_target::ReferenceScope::NoCheckedSlot)
            .unwrap();
    }
}
