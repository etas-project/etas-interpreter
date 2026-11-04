use etas_frontend::CheckedProgram;
use etas_types::TypeId;

mod evidence;
#[cfg(test)]
mod tests;

use evidence::{ActionEvidence, NativeActionOrigin};

/// An invocation checked in one program universe. Only the evaluator and the
/// checked revision bridge can construct this proof; HIR IDs are not portable.
#[derive(Clone, Debug)]
pub struct CheckedActionInvocation {
    pub(crate) program: CheckedProgram,
    pub(crate) effect: etas_effects::Effect,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) output: TypeId,
    pub(crate) returns_never: bool,
    evidence: ActionEvidence,
}

impl CheckedActionInvocation {
    pub(crate) fn evidence_identity(&self) -> Result<usize, String> {
        self.evidence.identity()
    }
    pub(crate) fn checkpoint_origin(
        &self,
        revisions: &mut std::collections::BTreeMap<
            etas_code::RevisionKey,
            std::sync::Arc<crate::api::code::PreparedCodeRevision>,
        >,
    ) -> Result<crate::orchestration::ActionInvocationSnapshot, String> {
        self.validate_origin()?;
        self.evidence.checkpoint(revisions)
    }

    pub(crate) fn restore_origin(
        snapshot: &crate::orchestration::ActionInvocationSnapshot,
        revisions: &crate::api::code::PreparedRevisionSet,
    ) -> Result<Self, String> {
        use crate::orchestration::{ActionOriginSnapshot, CallTargetSnapshot};
        let crate::orchestration::ActionInvocationSnapshot::Native { native, origin } = snapshot
        else {
            return Err("forwarded action evidence requires its checked suspension graph".into());
        };
        let execution = revisions.execution_snapshot(native)?;
        let proof = match origin {
            ActionOriginSnapshot::Source {
                expr,
                bindings,
                type_parameter_bindings,
                effect_row_bindings,
            } => {
                let map = crate::control::GenericBindings {
                    types: bindings.iter().cloned().collect(),
                    parameters: type_parameter_bindings.iter().copied().collect(),
                    rows: effect_row_bindings.iter().cloned().collect(),
                };
                if map.types.len() != bindings.len()
                    || map.parameters.len() != type_parameter_bindings.len()
                    || map.rows.len() != effect_row_bindings.len()
                {
                    return Err("action origin has duplicate type bindings".into());
                }
                Self::source(execution, *expr, &map)?
            }
            ActionOriginSnapshot::Standard {
                call:
                    CallTargetSnapshot::StdIntrinsic {
                        origin,
                        substitutions,
                        intrinsic,
                        dispatch,
                        parameter_types,
                        result_type,
                    },
            } => Self::standard(
                execution,
                &crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
                    origin: *origin,
                    substitutions: substitutions.clone(),
                    identity: crate::intrinsic::dispatch::StdIntrinsicIdentity {
                        intrinsic: *intrinsic,
                        dispatch: *dispatch,
                    },
                    parameter_types: parameter_types.clone(),
                    result_type: *result_type,
                },
            )?,
            ActionOriginSnapshot::Standard { .. } => {
                return Err("action origin is not a checked standard invocation".into());
            }
        };
        proof.validate_origin()?;
        Ok(proof)
    }
    pub(crate) fn source(
        execution: crate::value::CheckedExecutionProgram,
        expr: etas_hir::HirExprId,
        bindings: &crate::control::GenericBindings,
    ) -> Result<Self, String> {
        let abi = crate::plan::checked_action::source_action(&execution, expr, bindings)?;
        let program = execution.checked.clone();
        Ok(Self::with_abi(
            program,
            abi,
            ActionEvidence::native(
                execution,
                NativeActionOrigin::Source {
                    expr,
                    bindings: bindings.clone(),
                },
            ),
        ))
    }

    pub(crate) fn standard(
        execution: crate::value::CheckedExecutionProgram,
        call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
    ) -> Result<Self, String> {
        let abi = crate::plan::checked_action::standard_action(&execution, call)?;
        let program = execution.checked.clone();
        Ok(Self::with_abi(
            program,
            abi,
            ActionEvidence::native(
                execution,
                NativeActionOrigin::Standard(Box::new(call.clone())),
            ),
        ))
    }

    fn with_abi(
        program: CheckedProgram,
        abi: crate::plan::checked_action::CheckedActionAbi,
        evidence: ActionEvidence,
    ) -> Self {
        Self {
            program,
            effect: abi.effect,
            parameters: abi.parameters,
            output: abi.output,
            returns_never: abi.returns_never,
            evidence,
        }
    }

    pub(crate) fn project(
        &self,
        edge: &crate::plan::code::RevisionProjection,
    ) -> Result<Self, String> {
        if !std::sync::Arc::ptr_eq(&self.program, edge.source()) {
            return Err(
                "action projection is not contiguous with its checked source program".into(),
            );
        }
        let abi = crate::plan::checked_action::CheckedActionAbi::project_parts(
            &self.effect,
            &self.parameters,
            self.output,
            self.returns_never,
            edge,
        )?;
        Ok(Self::with_abi(
            edge.target().clone(),
            abi,
            self.evidence.project(edge.clone())?,
        ))
    }

    /// Rebuild from native checked facts, then replay only pinned revision edges.
    /// The recorded action/type tuple is not itself proof of a resumable ABI.
    pub(crate) fn validate_origin(&self) -> Result<(), String> {
        let (program, abi) = self.evidence.resolve()?;
        if !std::sync::Arc::ptr_eq(&program, &self.program)
            || abi.effect != self.effect
            || abi.parameters != self.parameters
            || abi.output != self.output
            || abi.returns_never != self.returns_never
        {
            return Err("action invocation ABI differs from its checked origin".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum DefaultActionBoundary {
    Console(Box<super::PendingConsole>),
    Native {
        kind: crate::intrinsic::dispatch::StdCallable,
        call: crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        args: Vec<crate::value::InterpValue>,
        type_bindings: std::collections::HashMap<String, TypeId>,
        span: etas_core::Span,
    },
    StreamRead {
        stream: crate::value::InterpValue,
        max_bytes: usize,
        timeout_ms: Option<u64>,
        span: etas_core::Span,
    },
    FileReadLimited {
        path: crate::value::InterpValue,
        max_bytes: usize,
        span: etas_core::Span,
    },
    DirectoryListLimited {
        path: crate::value::InterpValue,
        max_entries: usize,
        max_bytes: usize,
        span: etas_core::Span,
    },
}

#[derive(Clone, Debug)]
pub struct FileReadReduction {
    pub(crate) call: crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
    pub(crate) args: Vec<crate::value::InterpValue>,
    pub(crate) limit: usize,
    pub(crate) span: etas_core::Span,
}

#[derive(Clone, Debug)]
pub struct DirectoryListReduction {
    pub(crate) call: crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
    pub(crate) args: Vec<crate::value::InterpValue>,
    pub(crate) max_entries: usize,
    pub(crate) max_bytes: usize,
    pub(crate) span: etas_core::Span,
}

#[derive(Clone, Debug)]
pub struct StreamReadReduction {
    pub(crate) call: crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
    pub(crate) args: Vec<crate::value::InterpValue>,
    pub(crate) limit: usize,
    pub(crate) deadline_unix_ms: Option<u64>,
    pub(crate) body: crate::value::BytesValue,
    pub(crate) span: etas_core::Span,
}
