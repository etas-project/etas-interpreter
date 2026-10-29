use std::sync::Arc;

use super::{RevisionProjection, convert_scoped};
use crate::{
    control::CheckedActionInvocation,
    value::{CheckedExecutionProgram, InterpValue},
};

#[derive(Clone, Debug)]
pub(crate) struct ActionProjection {
    steps: Vec<(
        RevisionProjection,
        CheckedActionInvocation,
        CheckedActionInvocation,
    )>,
    source: CheckedActionInvocation,
    target: CheckedActionInvocation,
}

impl ActionProjection {
    pub(crate) fn source_identity(&self) -> Result<usize, String> {
        self.source.evidence_identity()
    }
    pub(crate) fn checkpoint_origin(
        &self,
        revisions: &mut std::collections::BTreeMap<
            etas_code::RevisionKey,
            Arc<crate::api::code::PreparedCodeRevision>,
        >,
    ) -> Result<crate::orchestration::ActionInvocationSnapshot, String> {
        self.source.checkpoint_origin(revisions)
    }
    pub(crate) fn new(
        source: CheckedActionInvocation,
        edges: Vec<RevisionProjection>,
        target: &CheckedExecutionProgram,
    ) -> Result<Self, String> {
        let mut current = source.clone();
        let mut steps = Vec::with_capacity(edges.len());
        for edge in edges {
            let next = current.project(&edge)?;
            steps.push((edge, current, next.clone()));
            current = next;
        }
        if !Arc::ptr_eq(&current.program, &target.checked) {
            return Err("action projection does not reach the parent checked program".into());
        }
        Ok(Self {
            source,
            target: current,
            steps,
        })
    }

    pub(crate) fn target(&self) -> &CheckedActionInvocation {
        &self.target
    }

    pub(crate) fn arguments(
        &self,
        mut args: Vec<InterpValue>,
        execution: &CheckedExecutionProgram,
    ) -> Result<Vec<InterpValue>, String> {
        if !Arc::ptr_eq(&execution.checked, &self.source.program)
            || args.len() != self.source.parameters.len()
        {
            return Err("action payload disagrees with its checked native invocation".into());
        }
        for (edge, from, to) in &self.steps {
            args = args
                .into_iter()
                .zip(from.parameters.iter().zip(&to.parameters))
                .map(|(value, (a, b))| {
                    convert_scoped(
                        &from.program,
                        &to.program,
                        *a,
                        *b,
                        value,
                        Some(execution),
                        Some(edge.clone()),
                    )
                    .map_err(|error| error.to_string())
                })
                .collect::<Result<_, _>>()?;
        }
        Ok(args)
    }

    pub(crate) fn resume(
        &self,
        mut value: InterpValue,
        execution: &CheckedExecutionProgram,
    ) -> Result<InterpValue, String> {
        if self.source.returns_never || !Arc::ptr_eq(&execution.checked, &self.target.program) {
            return Err("action cannot resume under this checked invocation".into());
        }
        for (edge, from, to) in self.steps.iter().rev() {
            value = convert_scoped(
                &to.program,
                &from.program,
                to.output,
                from.output,
                value,
                Some(execution),
                Some(edge.reversed()),
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(value)
    }
}
