use std::sync::Arc;

use etas_core::{AnalysisDiagnosticCode, Span};
use etas_frontend::code::definition::FrozenDefinitionEnvironment;

use crate::{
    control::{CallTarget, Continuation, ExecutionFault, PendingPerform},
    eval::EvalContext,
    value::InterpValue,
};

impl EvalContext<'_> {
    pub(in crate::eval) fn compile_handler_result_guard(
        &self,
        perform: &PendingPerform,
    ) -> Result<Option<Continuation>, ExecutionFault> {
        let Some(etas_effects::Effect::Action(action)) = self.checked_perform_action(perform)?
        else {
            return Ok(None);
        };
        let Some(target) = self.plan.dispatch.compiler_action(&action) else {
            return Ok(None);
        };
        if target.operation != etas_host::compiler::CompilerOperation::Compile {
            return Ok(None);
        }
        let [definition @ (InterpValue::AgentDefinition(_) | InterpValue::CallableDefinition(_))] =
            perform.args.as_slice()
        else {
            return Err(missing(
                "compile handler requires its checked typed definition argument",
                perform.span,
            ));
        };
        Ok(Some(Continuation::ValidateCompileResult {
            definition: Box::new(definition.clone()),
            span: perform.span,
        }))
    }

    pub(in crate::eval) fn validate_compile_handler_result(
        &self,
        definition: &InterpValue,
        value: &InterpValue,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        let (program, backend, reference) = match definition {
            InterpValue::AgentDefinition(value) => {
                (value.program(), value.backend(), value.definition())
            }
            InterpValue::CallableDefinition(value) => (
                value.program(),
                value.backend(),
                value.definition().map_err(|reason| missing(reason, span))?,
            ),
            _ => {
                return Err(missing(
                    "compile result guard has no native typed definition",
                    span,
                ));
            }
        };
        if !Arc::ptr_eq(program, self.checked) {
            return Err(missing(
                "compile result guard belongs to a different checked program",
                span,
            ));
        }
        let view = backend
            .resolve(reference)
            .map_err(|error| missing(error.to_string(), span))?;
        let expected = view
            .environment()
            .map_err(|error| missing(error.to_string(), span))?;
        validate_value(expected, self.checked, value).map_err(|reason| missing(reason, span))
    }
}

fn validate_value(
    expected: &FrozenDefinitionEnvironment,
    program: &etas_frontend::CheckedProgram,
    value: &InterpValue,
) -> Result<(), String> {
    let InterpValue::Callable(target) = value else {
        return Err("compile handler returned a non-callable value".into());
    };
    let mut target = target;
    let mut execution = program;
    loop {
        match target {
            CallTarget::Specialized { target: inner, .. }
            | CallTarget::Limited { target: inner, .. } => target = inner,
            CallTarget::Scoped(scoped) => {
                if let Some(prepared) = &scoped.program().revision
                    && matches!(scoped.target_ref(), CallTarget::FlowItem(item) if *item == prepared.entry().item)
                    && same_contract(expected, prepared.frozen_environment())
                {
                    return Ok(());
                }
                execution = &scoped.program().checked;
                target = scoped.target_ref();
            }
            CallTarget::AgentRevision(agent) => {
                if agent.is_default_entry()
                    && same_contract(expected, agent.prepared().frozen_environment())
                {
                    return Ok(());
                }
                if let Some(item) = agent.frozen_agent()
                    && expected.identity().kind() == etas_code::DefinitionKind::Agent
                    && Arc::ptr_eq(expected.checked(), agent.destination_program())
                    && expected.declaration() == item
                {
                    return Ok(());
                }
                break;
            }
            CallTarget::FlowItem(item)
                if expected.identity().kind() == etas_code::DefinitionKind::Flow
                    && Arc::ptr_eq(expected.checked(), execution)
                    && expected.default_body() == Some(*item) =>
            {
                return Ok(());
            }
            CallTarget::AgentItem(item)
                if expected.identity().kind() == etas_code::DefinitionKind::Agent
                    && Arc::ptr_eq(expected.checked(), execution)
                    && expected.declaration() == *item =>
            {
                return Ok(());
            }
            _ => break,
        }
    }
    Err("compile handler returned a callable without matching checked revision and frozen-contract provenance".into())
}

fn same_contract(
    expected: &FrozenDefinitionEnvironment,
    actual: &FrozenDefinitionEnvironment,
) -> bool {
    expected.identity() == actual.identity()
        && expected.fingerprint() == actual.fingerprint()
        && expected.contract().fingerprint() == actual.contract().fingerprint()
}

fn missing(reason: impl Into<String>, span: Span) -> ExecutionFault {
    ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_frontend::{
        Frontend, ProjectInput, SourceInput,
        code::{
            definition::{DefinitionCatalog, ReflectionOptions},
            identity::DefinitionOrigins,
        },
    };
    use std::collections::BTreeMap;

    #[test]
    fn equal_flow_types_do_not_prove_the_requested_frozen_contract() {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(
            "module app; flow target() -> unit ![] { return; } flow other() -> unit ![] { return; } flow inspect() -> unit { let d = ^target; return; }",
        ));
        input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
        input.options.reflection = ReflectionOptions::default();
        let output = Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let package = etas_code::PackageKey::from_canonical_fingerprint(
            etas_code::CodeFingerprint::of_bytes("etas.test.compile-result-guard", b"app"),
        );
        let origins = DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
            standard_package: package,
            external_definitions: BTreeMap::new(),
        };
        let catalog =
            DefinitionCatalog::build(checked.clone(), &origins, Default::default()).unwrap();
        let reflected = *checked.types.reflections.keys().next().unwrap();
        let frozen = catalog.freeze_flow(reflected).unwrap();
        let original = frozen.default_body().unwrap();
        let other = checked
            .hir
            .items
            .iter()
            .find_map(|(item, data)| {
                data.symbol()
                    .and_then(|symbol| checked.symbols.get(symbol))
                    .filter(|symbol| symbol.name == "other")
                    .map(|_| item)
            })
            .unwrap();
        assert!(
            validate_value(
                &frozen,
                &checked,
                &InterpValue::Callable(CallTarget::FlowItem(original))
            )
            .is_ok()
        );
        assert!(
            validate_value(
                &frozen,
                &checked,
                &InterpValue::Callable(CallTarget::FlowItem(other))
            )
            .is_err()
        );
        assert!(validate_value(&frozen, &checked, &InterpValue::Unit).is_err());
    }
}
