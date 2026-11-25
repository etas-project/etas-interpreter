use super::project::Projector;
use crate::{
    control::{ControlSignal, ExecutionFault, PendingPerform},
    eval::EvalContext,
    value::InterpValue,
};
use etas_core::{AnalysisDiagnosticCode, Span};
use etas_host::compiler::{CompilerFailure, CompilerRequest, CompilerResult};

#[derive(Clone)]
pub(crate) struct CompilerInvocation {
    pub request: CompilerRequest,
    pub definitions: std::sync::Arc<etas_frontend::code::definition::NativeDefinitionStore>,
    pub result_type: etas_types::TypeId,
    pub error_type: etas_types::TypeId,
}

impl std::fmt::Debug for CompilerInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompilerInvocation")
            .field("request", &self.request.id)
            .field("result_type", &self.result_type)
            .finish_non_exhaustive()
    }
}

impl EvalContext<'_> {
    pub(in crate::eval) fn compiler_action_uses_caller_context(
        &self,
        perform: &PendingPerform,
    ) -> bool {
        if !matches!(
            self.source_default_signature(perform),
            Some(("Code", "compile" | "check"))
        ) {
            return false;
        }
        let [value] = perform.args.as_slice() else {
            return false;
        };
        crate::value::source_code::Definition::input(value, self.checked).is_ok_and(|definition| {
            !std::sync::Arc::ptr_eq(
                definition.backend().catalog().checked_program(),
                self.checked,
            )
        })
    }

    pub(crate) fn compiler_invocation(
        &mut self,
        perform: &PendingPerform,
    ) -> Result<Option<CompilerInvocation>, ExecutionFault> {
        let missing = |reason: &str| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                perform.span,
                reason,
            )
        };
        let Some(effect) = self.checked_perform_action(perform)? else {
            return Ok(None);
        };
        let action = match effect {
            etas_effects::Effect::Action(action) => action,
            etas_effects::Effect::AppliedAction(instance) if instance.args.is_empty() => {
                instance.action
            }
            _ => return Ok(None),
        };
        let Some(target) = self.plan.dispatch.compiler_action(&action) else {
            return Ok(None);
        };
        if !self.plan.action_mediation.has_default_action(&action) {
            return Err(missing(
                "compiler default handling is absent from checked action mediation",
            ));
        }
        let operation = target.operation;
        let signature = self
            .checked
            .types
            .qualified_action_signatures
            .get(&target.signature_path)
            .ok_or_else(|| missing("compiler action has no checked canonical signature"))?;
        let error_type = signature
            .default_handler_effects
            .as_ref()
            .and_then(|row| match row.effects.as_slice() {
                [effect] if effect.name == "Error" && row.tail.is_none() => {
                    match effect.args.as_slice() {
                        [etas_types::EffectArgRef::Type(ty)] => Some(*ty),
                        _ => None,
                    }
                }
                _ => None,
            })
            .ok_or_else(|| missing("compiler action has no checked service error contract"))?;
        let result_type = if let Some(call) = &perform.forwarded_call {
            call.result_type
        } else if let Some(invocation) = &perform.checked_action {
            invocation.output
        } else {
            *perform
                .expr
                .and_then(|expr| self.checked.types.expr_types.get(&expr))
                .ok_or_else(|| missing("compiler action has no checked call result ABI"))?
        };
        let [value] = perform.args.as_slice() else {
            return Err(missing(
                "compiler action requires one checked definition payload",
            ));
        };
        let retained = crate::value::source_code::Definition::input(value, self.checked)
            .map_err(|reason| missing(&reason))?;
        let definitions = retained.backend().clone();
        let definition = retained
            .reference()
            .map_err(|reason| missing(&reason))?
            .clone();
        Ok(Some(CompilerInvocation {
            definitions,
            request: CompilerRequest {
                id: self.next_host_request_id(),
                operation,
                definition,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            result_type,
            error_type,
        }))
    }

    pub(crate) fn resume_compiler_result(
        &mut self,
        invocation: &CompilerInvocation,
        perform: PendingPerform,
        result: Result<CompilerResult, CompilerFailure>,
        prepared: Option<std::sync::Arc<crate::api::code::PreparedCodeRevision>>,
    ) -> ControlSignal {
        match result {
            Ok(CompilerResult::Checked(report)) => match super::report::check_report(
                &self.checked.type_store,
                &self.checked.types.known_std_types.code,
                &self.checked.types.enum_layouts,
                invocation.result_type,
                &report,
            ) {
                Ok(value) => self.resume_perform_signal(perform, value),
                Err(reason) => ControlSignal::missing_checked_fact(reason, perform.span),
            },
            Ok(CompilerResult::Compiled(reference)) => {
                let Some(prepared) = prepared else {
                    return ControlSignal::missing_checked_fact(
                        "compiled code has no prepared native revision",
                        perform.span,
                    );
                };
                if prepared.snapshot_fingerprint() != reference.snapshot_fingerprint()
                    || prepared.identity() != reference.identity()
                    || prepared.revision() != reference.revision()
                {
                    return ControlSignal::missing_checked_fact(
                        "prepared code disagrees with the checked compiler response",
                        perform.span,
                    );
                }
                if prepared.identity().kind() == etas_code::DefinitionKind::Flow {
                    return match crate::value::ScopedCallableValue::from_revision(
                        prepared,
                        self.revision,
                        self.checked,
                        invocation.result_type,
                    ) {
                        Ok(value) => {
                            self.resume_perform_signal(perform, InterpValue::Callable(value))
                        }
                        Err(reason) => ControlSignal::missing_checked_fact(reason, perform.span),
                    };
                }
                match crate::value::AgentRevisionValue::new_in_program(
                    prepared,
                    self.revision.cloned(),
                    self.checked,
                    invocation.result_type,
                ) {
                    Ok(value) => self.resume_perform_signal(
                        perform,
                        InterpValue::Callable(crate::control::CallTarget::AgentRevision(value)),
                    ),
                    Err(reason) => ControlSignal::missing_checked_fact(reason, perform.span),
                }
            }
            Err(failure) => {
                let (kind, diagnostics) = match failure {
                    CompilerFailure::Rejected(report) => ("rejected", report.diagnostics.clone()),
                    CompilerFailure::Service { diagnostics } => ("service", diagnostics),
                };
                let project = Projector {
                    store: &self.checked.type_store,
                    known: &self.checked.types.known_std_types.code,
                    enums: &self.checked.types.enum_layouts,
                };
                let value = (|| {
                    let item = project
                        .array_element(project.field(invocation.error_type, "diagnostics")?)?;
                    let diagnostics = diagnostics
                        .into_iter()
                        .map(|value| project.diagnostic(item, value))
                        .collect::<Result<Vec<_>, String>>()?;
                    project.record(
                        invocation.error_type,
                        vec![
                            ("kind", InterpValue::String(kind.into())),
                            ("diagnostics", InterpValue::Array(diagnostics.into())),
                        ],
                    )
                })();
                match value {
                    Ok(value) => self.propagate_perform_signal(PendingPerform {
                        checked_action: None,
                        default_boundary: None,
                        forwarded_call: None,
                        expr: None,
                        inference: None,
                        action: error_action(perform.span),
                        error_type: Some(invocation.error_type),
                        args: vec![value],
                        span: perform.span,
                        continuation: perform.continuation,
                        residual: None,
                    }),
                    Err(reason) => ControlSignal::missing_checked_fact(reason, perform.span),
                }
            }
        }
    }
}

fn error_action(span: Span) -> etas_hir::ResolvedActionRef {
    etas_hir::ResolvedActionRef {
        effect: etas_hir::HirEffectRef {
            path: etas_hir::unresolved_path_from_segments(&["Error"], span),
            args: Vec::new(),
            span,
        },
        action: "raise".into(),
        action_symbol: etas_hir::ResolveResult::Unresolved,
        span,
    }
}
