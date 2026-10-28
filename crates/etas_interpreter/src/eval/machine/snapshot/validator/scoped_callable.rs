use super::*;
use crate::orchestration::ScopedCallableMetadata;

impl SnapshotValidator<'_> {
    pub(crate) fn scoped_callable(
        &self,
        metadata: &ScopedCallableMetadata,
        target: &CallTargetSnapshot,
    ) -> Result<
        (
            crate::value::CheckedExecutionProgram,
            Vec<std::sync::Arc<crate::plan::code::CallableValueProjection>>,
        ),
        String,
    > {
        if metadata.retained_bytes() > self.limits.max_value_bytes as u64
            || metadata.projections.len() > self.limits.max_nodes
        {
            return Err("checkpoint scoped callable metadata exceeds resource limits".into());
        }
        let revisions = self
            .revisions
            .ok_or("checkpoint callable requires freshly prepared revisions")?;
        let projections = revisions.callable_projections(metadata, self.checked)?;
        let native = revisions.execution_callable(metadata, target)?;
        let mut validator = SnapshotValidator::new(
            &native.checked,
            &native.plan.slots,
            &native.plan.dispatch,
            &native.plan.closures,
            self.limits,
        )
        .with_revisions(self.revisions);
        validator.frame_owners = self.frame_owners.clone();
        validator.frame_definitions = self.frame_definitions.clone();
        validator.call_target(target, "checkpoint scoped callable native target")?;
        crate::value::ScopedCallableValue::checked_snapshot_root(
            &native,
            metadata.native_type,
            target,
        )?;
        Ok((native, projections))
    }
}
