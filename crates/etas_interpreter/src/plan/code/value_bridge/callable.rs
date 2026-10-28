use std::sync::Arc;

use etas_frontend::CheckedProgram;
use etas_types::{EffectArgRef, FlowType, TypeId};

use super::{RevisionValueBridgeError, convert_scoped};
use crate::value::{CheckedExecutionProgram, InterpValue};

/// A child projection of a proven frozen interface. Construction is private to
/// the value walker; callers cannot authorize a type pair by matching shapes.
#[derive(Debug)]
pub(crate) struct CallableValueProjection {
    source: CheckedProgram,
    target: CheckedProgram,
    from: TypeId,
    to: TypeId,
    source_signature: FlowType,
    target_signature: FlowType,
    revision: Option<super::RevisionProjection>,
    revision_owner: Option<Arc<crate::api::code::PreparedAgentRevision>>,
}

impl CallableValueProjection {
    pub(crate) fn action_projection(&self) -> Result<Option<super::RevisionProjection>, String> {
        match &self.revision {
            Some(edge) => Ok(Some(edge.clone())),
            None if Arc::ptr_eq(&self.source, &self.target) => Ok(None),
            None => Err("callable action projection has no checked revision edge".into()),
        }
    }
    pub(super) fn new(
        source: CheckedProgram,
        target: CheckedProgram,
        from: TypeId,
        to: TypeId,
        source_signature: FlowType,
        target_signature: FlowType,
        revision: Option<super::RevisionProjection>,
    ) -> Self {
        let revision_owner = revision.as_ref().and_then(|edge| match edge {
            super::RevisionProjection::Candidate(bridge)
            | super::RevisionProjection::Original(bridge) => bridge.runtime_owner(),
        });
        Self {
            source,
            target,
            from,
            to,
            source_signature,
            target_signature,
            revision,
            revision_owner,
        }
    }

    pub(crate) fn revision_owner(&self) -> Option<&Arc<crate::api::code::PreparedAgentRevision>> {
        self.revision_owner.as_ref()
    }

    pub(crate) fn restore_checked(
        edge: super::RevisionProjection,
        from: TypeId,
        to: TypeId,
    ) -> Result<Self, String> {
        if edge.runtime_type(from)? != to {
            return Err(
                "checkpoint callable projection differs from its checked type bridge".into(),
            );
        }
        let source = edge.source().clone();
        let target = edge.target().clone();
        let (Some(etas_types::Type::Function(left)), Some(etas_types::Type::Function(right))) =
            (source.type_store.get(from), target.type_store.get(to))
        else {
            return Err(
                "checkpoint callable projection does not name checked function types".into(),
            );
        };
        Ok(Self::new(
            source.clone(),
            target.clone(),
            from,
            to,
            left.clone(),
            right.clone(),
            Some(edge),
        ))
    }
    pub(crate) fn source(&self) -> (&CheckedProgram, TypeId) {
        (&self.source, self.from)
    }
    pub(crate) fn target(&self) -> (&CheckedProgram, TypeId) {
        (&self.target, self.to)
    }
    pub(crate) fn signature(&self) -> &FlowType {
        &self.source_signature
    }
    pub(crate) fn reverses(&self, previous: &Self) -> bool {
        Arc::ptr_eq(&self.source, &previous.target)
            && Arc::ptr_eq(&self.target, &previous.source)
            && self.from == previous.to
            && self.to == previous.from
    }
    pub(crate) fn argument_to_source(
        &self,
        index: usize,
        value: InterpValue,
        execution: &CheckedExecutionProgram,
    ) -> Result<InterpValue, RevisionValueBridgeError> {
        let Some((&target, &source)) = self
            .target_signature
            .input
            .get(index)
            .zip(self.source_signature.input.get(index))
        else {
            return Err(super::error(
                self.from,
                self.to,
                super::RevisionValueFailure::InconsistentLayout,
            ));
        };
        convert_scoped(
            &self.target,
            &self.source,
            target,
            source,
            value,
            Some(execution),
            self.revision
                .as_ref()
                .map(super::RevisionProjection::reversed),
        )
    }
    pub(crate) fn result_to_target(
        &self,
        value: InterpValue,
        execution: &CheckedExecutionProgram,
    ) -> Result<InterpValue, RevisionValueBridgeError> {
        convert_scoped(
            &self.source,
            &self.target,
            self.source_signature.output,
            self.target_signature.output,
            value,
            Some(execution),
            self.revision.clone(),
        )
    }

    pub(crate) fn error_to_target(
        &self,
        error_type: TypeId,
        value: InterpValue,
        execution: &CheckedExecutionProgram,
    ) -> Result<(TypeId, InterpValue), RevisionValueBridgeError> {
        let invalid = |reason: String| {
            super::error(
                self.from,
                self.to,
                super::RevisionValueFailure::InvalidCallable(reason),
            )
        };
        let (source, target) = self
            .source_signature
            .effects
            .as_ref()
            .zip(self.target_signature.effects.as_ref())
            .ok_or_else(|| {
                invalid("typed error projection requires checked callable effect rows".into())
            })?;
        if source.tail.is_some()
            || target.tail.is_some()
            || source.effects.len() != target.effects.len()
        {
            return Err(invalid(
                "typed error projection requires closed, corresponding callable rows".into(),
            ));
        }
        // These rows are child projections of the same frozen descriptor.
        // Only its checked Error payload pair may cross this callable boundary.
        let mut projected = None;
        for (from, to) in source.effects.iter().zip(&target.effects) {
            if from.name != to.name || from.args.len() != to.args.len() {
                return Err(invalid(
                    "projected callable effect identities disagree".into(),
                ));
            }
            if from.name != "Error" {
                continue;
            }
            let ([EffectArgRef::Type(from)], [EffectArgRef::Type(to)]) =
                (from.args.as_slice(), to.args.as_slice())
            else {
                return Err(invalid(
                    "projected Error row has no single checked type selector".into(),
                ));
            };
            if *from == error_type {
                if projected.is_some_and(|previous| previous != *to) {
                    return Err(invalid(
                        "typed error has ambiguous callable projection".into(),
                    ));
                }
                projected = Some(*to);
            }
        }
        let target_type = projected.ok_or_else(|| {
            invalid(format!(
                "typed error {error_type:?} is absent from the checked callable row {source:?}"
            ))
        })?;
        let value = convert_scoped(
            &self.source,
            &self.target,
            error_type,
            target_type,
            value,
            Some(execution),
            self.revision.clone(),
        )?;
        Ok((target_type, value))
    }
}
