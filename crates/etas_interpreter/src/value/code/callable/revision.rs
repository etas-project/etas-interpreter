use super::*;
use crate::{api::code::PreparedCodeRevision, plan::code::RevisionProjection};

impl ScopedCallableValue {
    pub(crate) fn from_revision(
        prepared: Arc<PreparedCodeRevision>,
        caller_revision: Option<&Arc<PreparedCodeRevision>>,
        caller: &CheckedProgram,
        expected: TypeId,
    ) -> Result<CallTarget, String> {
        if prepared.identity().kind() != etas_code::DefinitionKind::Flow
            || !matches!(caller.type_store.get(expected), Some(Type::Function(_)))
        {
            return Err("compiled flow requires its checked ordinary callable result ABI".into());
        }
        let original = prepared.type_bridge().original_program();
        let original_type = match caller_revision {
            Some(source) => {
                if !Arc::ptr_eq(original, source.type_bridge().original_program()) {
                    return Err(
                        "compiled flow and caller have no common frozen source environment".into(),
                    );
                }
                source
                    .type_bridge()
                    .runtime_to_original(caller, expected)
                    .map_err(|error| error.to_string())?
            }
            None if Arc::ptr_eq(original, caller) => expected,
            None => {
                return Err("compiled flow belongs to a different checked source program".into());
            }
        };
        let native_type = prepared
            .type_bridge()
            .runtime_to_candidate(original, original_type)
            .map_err(|error| error.to_string())?;
        prepared.type_bridge().bind_runtime_owner(&prepared);
        let execution = CheckedExecutionProgram {
            checked: prepared.program().clone(),
            plan: prepared.plan().clone(),
            definitions: Some(prepared.definitions.clone()),
            revision: Some(prepared.clone()),
        };
        let mut target = CallTarget::FlowItem(prepared.entry().item);
        if let Some(application) = prepared.frozen_environment().callable_application() {
            if !Arc::ptr_eq(application.program(), original) {
                return Err("compiled application belongs to a different frozen program".into());
            }
            let Some(etas_types::ItemSignature::Flow(signature)) = prepared
                .program()
                .types
                .item_signatures
                .get(&prepared.entry().item)
            else {
                return Err("compiled application has no checked flow signature".into());
            };
            if signature.generic_params.len() != application.arguments().len() {
                return Err("compiled application has inconsistent generic arguments".into());
            }
            let mut type_bindings = Vec::new();
            let mut effect_row_bindings = Vec::new();
            for (parameter, argument) in
                signature.generic_params.iter().zip(application.arguments())
            {
                match (parameter.kind, argument) {
                    (
                        etas_types::CallableGenericParamKind::Type,
                        etas_types::CallableGenericArg::Type(ty),
                    ) => {
                        type_bindings.push((
                            parameter.name.clone(),
                            prepared
                                .type_bridge()
                                .runtime_to_candidate(original, *ty)
                                .map_err(|error| error.to_string())?,
                        ));
                    }
                    (
                        etas_types::CallableGenericParamKind::Effect,
                        etas_types::CallableGenericArg::EffectRow(row),
                    ) => {
                        effect_row_bindings.push((
                            parameter.name.clone(),
                            prepared
                                .type_bridge()
                                .runtime_row_to_candidate(original, row.clone())
                                .map_err(|error| error.to_string())?,
                        ));
                    }
                    _ => return Err("compiled application has wrong-kind generic argument".into()),
                }
            }
            target = CallTarget::Specialized {
                target: target.into(),
                type_bindings,
                effect_row_bindings,
            };
        }
        let native = InterpValue::Callable(target);
        let (mut ty, mut value) = RevisionProjection::Original(prepared.type_bridge().clone())
            .value(native_type, native, &execution)?;
        if let Some(source) = caller_revision {
            (ty, value) = RevisionProjection::Candidate(source.type_bridge().clone())
                .value(ty, value, &execution)?;
        }
        if ty != expected {
            return Err(
                "compiled flow projection differs from the checked invocation result".into(),
            );
        }
        match value {
            InterpValue::Callable(CallTarget::Scoped(value)) => Ok(CallTarget::Scoped(value)),
            _ => Err("compiled flow lost its native execution owner during ABI projection".into()),
        }
    }
}
