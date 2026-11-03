use super::*;
use crate::value::{AgentDefinitionValue, CallableDefinitionValue};
use etas_frontend::code::definition::StaticReflectionView;

mod revision;
mod runtime;

impl EvalContext<'_> {
    pub(super) fn eval_reflection(
        &mut self,
        expr: HirExprId,
        span: Span,
        frame: &Frame,
    ) -> ControlSignal {
        if let Some(signal) = self.code_operation_boundary(span) {
            return signal;
        }
        let Some(prepared) = &self.code_definitions else {
            return ControlSignal::missing_checked_fact(
                "reflection requires prepared canonical definition origins",
                span,
            );
        };
        if !prepared.is_for(self.checked) {
            return ControlSignal::missing_checked_fact(
                "reflection catalog belongs to a different checked program",
                span,
            );
        }
        if self.checked.reflection_site(expr).is_none() {
            return ControlSignal::missing_checked_fact(
                "reflection expression has no resolved source provenance",
                span,
            );
        }
        let Some(fact) = self.checked.checked_reflections().get(&expr) else {
            return ControlSignal::missing_checked_fact(
                "reflection expression has no checked target",
                span,
            );
        };
        if let etas_types::CheckedReflectionTarget::CallableValue { binding, signature } =
            fact.target
        {
            let value = match frame.get(binding) {
                Some(value) => value,
                None => match self.eval_global_symbol(binding, span) {
                    Ok(value) => value,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                },
            };
            let descriptor_type = match self.materialized_type(
                fact.descriptor_type,
                frame.generic_bindings(),
                span,
            ) {
                Ok(ty) => ty,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
            let signature = match self.materialized_type(signature, frame.generic_bindings(), span)
            {
                Ok(ty) => ty,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
            let lexical = match self.reflection_lexical_application(expr, frame) {
                Ok(lexical) => lexical,
                Err(error) => return error.into_signal(span),
            };
            return match self.reflect_callable_binding(
                expr,
                descriptor_type,
                signature,
                value,
                lexical.as_ref(),
            ) {
                Ok(value) => self
                    .code_operation_boundary(span)
                    .unwrap_or(ControlSignal::Value(value)),
                Err(error) => error.into_signal(span),
            };
        }
        if let etas_types::CheckedReflectionTarget::AgentValue { binding, .. } = fact.target {
            let value = match frame.get(binding) {
                Some(value) => value,
                None => match self.eval_global_symbol(binding, span) {
                    Ok(value) => value,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                },
            };
            return match self.reflect_agent_binding(expr, fact.descriptor_type, value) {
                Ok(value) => self
                    .code_operation_boundary(span)
                    .unwrap_or(ControlSignal::Value(value)),
                Err(error) => error.into_signal(span),
            };
        }
        if self
            .checked
            .types
            .generic_instantiations
            .contains_key(&expr)
            && matches!(
                fact.target,
                etas_types::CheckedReflectionTarget::Callable { .. }
                    | etas_types::CheckedReflectionTarget::ExternalCallable { .. }
            )
        {
            let descriptor_type = match self.materialized_type(
                fact.descriptor_type,
                frame.generic_bindings(),
                span,
            ) {
                Ok(ty) => ty,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
            let lexical = match self.reflection_lexical_application(expr, frame) {
                Ok(lexical) => lexical,
                Err(error) => return error.into_signal(span),
            };
            let external = match prepared
                .definitions()
                .catalog()
                .is_external_callable_reflection(expr)
            {
                Ok(external) => external,
                Err(error) => return ControlSignal::missing_checked_fact(error.to_string(), span),
            };
            if external {
                return match self.reflect_external_application(
                    expr,
                    descriptor_type,
                    lexical.as_ref(),
                ) {
                    Ok(value) => self
                        .code_operation_boundary(span)
                        .unwrap_or(ControlSignal::Value(value)),
                    Err(error) => error.into_signal(span),
                };
            }
            let Some(etas_hir::HirExpr::Reflect(reflection)) = self.checked.hir.exprs.get(expr)
            else {
                return ControlSignal::missing_checked_fact("reflection source is missing", span);
            };
            let etas_hir::HirReflectTarget::Path(path) = &reflection.target else {
                return ControlSignal::missing_checked_fact(
                    "generic flow reflection is not a path",
                    span,
                );
            };
            let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
                return ControlSignal::missing_checked_fact(
                    "generic flow reflection is unresolved",
                    span,
                );
            };
            let value = match self.eval_global_symbol(symbol, span) {
                Ok(value) => value,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
            let InterpValue::Callable(target) = value else {
                return ControlSignal::missing_checked_fact(
                    "generic reflection target is not a callable",
                    span,
                );
            };
            let target = match self.specialize_call_target_for_call(expr, target, frame, span) {
                Ok(target) => target,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
            let Some(etas_types::Type::Applied { args, .. }) =
                self.checked.type_store.get(descriptor_type)
            else {
                return ControlSignal::missing_checked_fact(
                    "generic reflection descriptor is not applied",
                    span,
                );
            };
            let [signature] = args.as_slice() else {
                return ControlSignal::missing_checked_fact(
                    "generic reflection descriptor has invalid arity",
                    span,
                );
            };
            let Some(etas_types::Type::Function(signature)) =
                self.checked.type_store.get(*signature)
            else {
                return ControlSignal::missing_checked_fact(
                    "generic reflection has no callable interface",
                    span,
                );
            };
            let application = match crate::value::ScopedCallableValue::checked_flow_application(
                self.checked,
                &target,
                signature,
            ) {
                Ok(application) => application,
                Err(error) => return ControlSignal::missing_checked_fact(error, span),
            };
            return match self.reflect_native_application_in_context(
                expr,
                descriptor_type,
                &application,
                lexical.as_ref(),
            ) {
                Ok(value) => self
                    .code_operation_boundary(span)
                    .unwrap_or(ControlSignal::Value(value)),
                Err(error) => error.into_signal(span),
            };
        }
        let backend = prepared.definitions();
        if self.revision.is_some() {
            return match self.reflect_frozen_source(expr, fact.descriptor_type, None) {
                Ok(value) => self
                    .code_operation_boundary(span)
                    .unwrap_or(ControlSignal::Value(value)),
                Err(error) => error.into_signal(span),
            };
        }
        let value = match backend.catalog().static_reflection(expr) {
            Ok(StaticReflectionView::Agent(_)) => match backend.reflect_agent(expr) {
                Ok(definition) => InterpValue::AgentDefinition(AgentDefinitionValue::new(
                    fact.descriptor_type,
                    definition,
                    backend.clone(),
                )),
                Err(error) => return code::definition_operation_failure(error, span),
            },
            Ok(StaticReflectionView::Callable(header)) => {
                let body = match backend
                    .catalog()
                    .retained_body(expr, backend.syntax_operations())
                {
                    Ok(body) => body,
                    Err(error) => {
                        return ControlSignal::missing_checked_fact(error.to_string(), span);
                    }
                };
                let definition = match CallableDefinitionValue::new(
                    fact.descriptor_type,
                    header.clone(),
                    body,
                    backend.clone(),
                ) {
                    Ok(definition) => definition,
                    Err(error) => return ControlSignal::missing_checked_fact(error, span),
                };
                let definition = if header
                    .id
                    .as_declaration()
                    .is_some_and(|key| key.kind() == etas_code::DefinitionKind::Flow)
                    && backend.has_flow_snapshot(&header.id)
                {
                    let snapshot = match backend.reflect_flow(expr) {
                        Ok(snapshot) => snapshot,
                        Err(error) => return code::definition_operation_failure(error, span),
                    };
                    match definition.with_definition(snapshot) {
                        Ok(definition) => definition,
                        Err(error) => return ControlSignal::missing_checked_fact(error, span),
                    }
                } else if header.id.as_method().is_some() {
                    let snapshot = match backend.reflect_method_snapshot(expr) {
                        Ok(snapshot) => snapshot,
                        Err(error) => return code::definition_operation_failure(error, span),
                    };
                    match definition.with_method_snapshot(snapshot) {
                        Ok(definition) => definition,
                        Err(error) => return ControlSignal::missing_checked_fact(error, span),
                    }
                } else {
                    definition
                };
                InterpValue::CallableDefinition(definition)
            }
            Err(error) => return ControlSignal::missing_checked_fact(error.to_string(), span),
        };
        self.code_operation_boundary(span)
            .unwrap_or(ControlSignal::Value(value))
    }
}
