use std::sync::Arc;

use crate::{
    api::code::PreparedCodeRevision,
    intrinsic::dispatch::CheckedStdIntrinsicCall,
    plan::{checked_action::CheckedActionAbi, code::RevisionProjection},
    value::CheckedExecutionProgram,
};

#[derive(Clone, Debug)]
pub(super) enum NativeActionOrigin {
    Source {
        expr: etas_hir::HirExprId,
        bindings: Vec<(String, etas_types::TypeId)>,
    },
    Standard(Box<CheckedStdIntrinsicCall>),
}

struct NativeActionEvidence {
    execution: CheckedExecutionProgram,
    origin: NativeActionOrigin,
}

enum EvidenceNode {
    Native(Box<NativeActionEvidence>),
    Projected {
        source: ActionEvidence,
        edge: RevisionProjection,
        owner: Arc<PreparedCodeRevision>,
    },
}

/// Shared immutable chain. It retains defining code, never invocation authority.
#[derive(Clone)]
pub(super) struct ActionEvidence(Option<Arc<EvidenceNode>>);

impl std::fmt::Debug for ActionEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionEvidence").finish_non_exhaustive()
    }
}

impl ActionEvidence {
    pub(super) fn identity(&self) -> Result<usize, String> {
        self.0
            .as_ref()
            .map(|node| Arc::as_ptr(node) as usize)
            .ok_or_else(|| "action invocation has no checked origin".into())
    }
    pub(super) fn checkpoint(
        &self,
        revisions: &mut std::collections::BTreeMap<
            etas_code::RevisionKey,
            Arc<PreparedCodeRevision>,
        >,
    ) -> Result<crate::orchestration::ActionInvocationSnapshot, String> {
        use crate::orchestration::{
            ActionInvocationSnapshot, ActionOriginSnapshot, CallTargetSnapshot,
        };
        let Some(EvidenceNode::Native(native)) = self.0.as_deref() else {
            return Err("projected action origin requires its owning suspended segment".into());
        };
        let origin = match &native.origin {
            NativeActionOrigin::Source { expr, bindings } => ActionOriginSnapshot::Source {
                expr: *expr,
                bindings: bindings.clone(),
            },
            NativeActionOrigin::Standard(call) => ActionOriginSnapshot::Standard {
                call: CallTargetSnapshot::StdIntrinsic {
                    origin: call.origin,
                    substitutions: call.substitutions.clone(),
                    intrinsic: call.identity.intrinsic,
                    dispatch: call.identity.dispatch,
                    parameter_types: call.parameter_types.clone(),
                    result_type: call.result_type,
                },
            },
        };
        Ok(ActionInvocationSnapshot::Native {
            native: native.execution.checkpoint_identity(revisions),
            origin,
        })
    }
    pub(super) fn native(execution: CheckedExecutionProgram, origin: NativeActionOrigin) -> Self {
        Self(Some(Arc::new(EvidenceNode::Native(Box::new(
            NativeActionEvidence { execution, origin },
        )))))
    }

    pub(super) fn project(&self, edge: RevisionProjection) -> Result<Self, String> {
        let owner = edge
            .runtime_owner()
            .ok_or("action projection has no pinned native revision owner")?;
        if !Arc::ptr_eq(owner.type_bridge().candidate_program(), owner.program()) {
            return Err("action projection has an inconsistent native revision owner".into());
        }
        Ok(Self(Some(Arc::new(EvidenceNode::Projected {
            source: self.clone(),
            edge,
            owner,
        }))))
    }

    pub(super) fn resolve(
        &self,
    ) -> Result<(etas_frontend::CheckedProgram, CheckedActionAbi), String> {
        let mut current = self;
        let mut edges = Vec::new();
        let native = loop {
            match current
                .0
                .as_deref()
                .ok_or("action invocation has no checked origin")?
            {
                EvidenceNode::Native(native) => break native,
                EvidenceNode::Projected {
                    source,
                    edge,
                    owner,
                } => {
                    let bridge = owner.type_bridge();
                    let candidate_edge = Arc::ptr_eq(edge.target(), bridge.candidate_program())
                        && Arc::ptr_eq(edge.source(), bridge.original_program());
                    let original_edge = Arc::ptr_eq(edge.source(), bridge.candidate_program())
                        && Arc::ptr_eq(edge.target(), bridge.original_program());
                    if !candidate_edge && !original_edge {
                        return Err(
                            "action projection does not belong to its pinned revision".into()
                        );
                    }
                    edges.push(edge);
                    current = source;
                }
            }
        };
        let mut abi = match &native.origin {
            NativeActionOrigin::Source { expr, bindings } => {
                let map = bindings
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashMap<_, _>>();
                if map.len() != bindings.len() {
                    return Err("action origin has duplicate type bindings".into());
                }
                crate::plan::checked_action::source_action(&native.execution, *expr, &map)?
            }
            NativeActionOrigin::Standard(call) => {
                crate::plan::checked_action::standard_action(&native.execution, call)?
            }
        };
        let mut program = native.execution.checked.clone();
        for edge in edges.into_iter().rev() {
            if !Arc::ptr_eq(&program, edge.source()) {
                return Err("action origin projection chain is not contiguous".into());
            }
            abi = abi.project(edge)?;
            program = edge.target().clone();
        }
        Ok((program, abi))
    }
}

impl Drop for ActionEvidence {
    fn drop(&mut self) {
        let mut current = self.0.take();
        while let Some(node) = current {
            match Arc::try_unwrap(node) {
                Ok(EvidenceNode::Projected { mut source, .. }) => current = source.0.take(),
                Ok(EvidenceNode::Native(_)) | Err(_) => break,
            }
        }
    }
}
