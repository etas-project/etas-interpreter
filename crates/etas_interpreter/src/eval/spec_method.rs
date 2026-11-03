use super::*;
use crate::control::ExecutionFault;

pub(super) struct SpecMethodCall<'a> {
    pub expr: HirExprId,
    pub receiver_expr: HirExprId,
    pub spec_path: &'a etas_hir::ResolvedPath,
    pub spec_args: &'a [HirTypeId],
    pub method: &'a str,
    pub args: &'a [HirArg],
    pub span: Span,
}

pub(super) struct SpecMethodDispatch<'a> {
    pub expr: HirExprId,
    pub receiver_expr: HirExprId,
    pub receiver: InterpValue,
    pub spec_symbol: SymbolId,
    pub spec_args: &'a [HirTypeId],
    pub method: &'a str,
    pub args: &'a [HirArg],
    pub span: Span,
}

impl<'a> EvalContext<'a> {
    pub(super) fn eval_spec_method_call(
        &mut self,
        call: SpecMethodCall<'_>,
        frame: &mut Frame,
    ) -> ControlSignal {
        let SpecMethodCall {
            expr,
            receiver_expr,
            spec_path,
            spec_args,
            method,
            args,
            span,
        } = call;
        let ResolveResult::Resolved(spec_symbol) = spec_path.resolution else {
            return ControlSignal::missing_checked_fact(
                "spec method selection is missing a resolved spec symbol",
                span,
            );
        };
        match self.eval_expr(receiver_expr, frame) {
            ControlSignal::Value(receiver) => self.eval_spec_method_on_receiver_value(
                SpecMethodDispatch {
                    expr,
                    receiver_expr,
                    receiver,
                    spec_symbol,
                    spec_args,
                    method,
                    args,
                    span,
                },
                frame,
            ),
            signal @ (ControlSignal::Apply(_)
            | ControlSignal::Checkpoint(_)
            | ControlSignal::Block(_)
            | ControlSignal::Expr(_)
            | ControlSignal::Call(_)
            | ControlSignal::Perform(_)
            | ControlSignal::Memory(_)
            | ControlSignal::Session(_)
            | ControlSignal::Console(_)
            | ControlSignal::Command(_)
            | ControlSignal::Model(_)
            | ControlSignal::Host(_)) => compose_signal_continuation(
                signal,
                Continuation::SpecMethodReceiver {
                    expr,
                    receiver_expr,
                    spec_symbol,
                    spec_args: spec_args.to_vec(),
                    method: method.to_owned(),
                    args: args.to_vec(),
                    span,
                    frame: frame.clone(),
                },
            ),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Cancelled(cause) => ControlSignal::Cancelled(cause),
            ControlSignal::Continue => ControlSignal::Continue,
        }
    }

    pub(super) fn eval_spec_method_on_receiver_value(
        &mut self,
        dispatch: SpecMethodDispatch<'_>,
        frame: &mut Frame,
    ) -> ControlSignal {
        let SpecMethodDispatch {
            expr,
            receiver_expr,
            receiver,
            spec_symbol,
            spec_args,
            method,
            args,
            span,
        } = dispatch;
        let target = match self.resolve_checked_spec_method_target(
            receiver_expr,
            spec_symbol,
            spec_args,
            method,
            frame.generic_bindings(),
            span,
        ) {
            Ok(target) => target,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        self.resume_call_args(
            CallArgsState {
                call: expr,
                target,
                args: args.into(),
                start_arg_index: 0,
                evaluated_args: vec![receiver],
                span,
            },
            frame,
        )
    }

    fn resolve_checked_spec_method_target(
        &self,
        receiver_expr: HirExprId,
        spec_symbol: SymbolId,
        spec_args: &[HirTypeId],
        method: &str,
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> Result<CallTarget, ExecutionFault> {
        let receiver_ty = self
            .checked
            .types
            .expr_types
            .get(&receiver_expr)
            .copied()
            .ok_or_else(|| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "spec method receiver is missing checked type facts",
                )
            })?;
        let receiver_ty = self.materialized_type(receiver_ty, bindings, span)?;
        let spec_symbol = self.checked_source_spec_symbol(spec_symbol, span)?;
        if !self
            .checked
            .types
            .spec_signatures
            .contains_key(&spec_symbol)
        {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "spec method selection is missing checked spec signature facts",
            ));
        }
        let spec_arg_tys = self.checked_spec_arg_types(spec_args, bindings, span)?;
        let mut matches = Vec::new();
        for implementation in &self.checked.types.spec_impls {
            let implementation_spec =
                self.checked_source_spec_symbol(implementation.spec_symbol, span)?;
            let same_spec = implementation_spec == spec_symbol
                || matches!(
                    (self.checked.types.external_specs.aliases.get(&implementation_spec),
                     self.checked.types.external_specs.aliases.get(&spec_symbol)),
                    (Some(left), Some(right)) if left == right
                );
            if !same_spec
                || !implementation_types_match(
                    &self.checked.type_store,
                    implementation.self_type,
                    receiver_ty,
                    &implementation.args,
                    &spec_arg_tys,
                )
            {
                continue;
            }
            matches.extend(
                implementation
                    .methods
                    .iter()
                    .filter(|candidate| candidate.name == method)
                    .map(|candidate| candidate.symbol),
            );
        }
        matches.sort_by_key(|symbol| symbol.0);
        matches.dedup();
        match matches.as_slice() {
            [symbol] => Ok(CallTarget::SpecImplMethod(*symbol)),
            [] => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!("spec method `{method}` is missing checked implementation method facts"),
            )),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("spec method `{method}` resolved to multiple implementation methods"),
            )),
        }
    }

    fn checked_source_spec_symbol(
        &self,
        symbol: SymbolId,
        span: Span,
    ) -> Result<SymbolId, ExecutionFault> {
        let mut current = symbol;
        let mut visited = std::collections::HashSet::new();
        while let Some(next) = self
            .checked
            .hir
            .source_import_targets
            .get(&current)
            .copied()
        {
            if !visited.insert(current) {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "spec method has a cyclic checked import chain",
                ));
            }
            current = next;
        }
        Ok(current)
    }

    fn checked_spec_arg_types(
        &self,
        spec_args: &[HirTypeId],
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> Result<Vec<etas_types::TypeId>, ExecutionFault> {
        let mut lowered = Vec::with_capacity(spec_args.len());
        for arg in spec_args {
            let Some(ty) = self.checked.types.type_refs.get(arg).copied() else {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "spec method type argument is missing checked type facts",
                ));
            };
            lowered.push(self.materialized_type(ty, bindings, span)?);
        }
        Ok(lowered)
    }

    pub(super) fn execute_spec_impl_method_call(
        &mut self,
        method_symbol: SymbolId,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let flow = match self.spec_impl_method_flow(method_symbol, span) {
            Ok(flow) => flow,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        if flow.params.len() != call_args.len() {
            let message = format!(
                "spec implementation method expects {} argument(s), got {}",
                flow.params.len(),
                call_args.len()
            );
            return ControlSignal::invalid_arguments(message, span);
        }
        let mut frame = match self.callable_frame(flow.scope, Default::default(), span) {
            Ok(frame) => frame,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        for (symbol, arg) in flow.params.iter().zip(call_args.into_iter()) {
            frame.insert(*symbol, arg);
        }
        let signal = self.execute_block(flow.body.block(), &mut frame);
        let signal = self.drive_restored_handlers(signal, &mut frame);
        match signal {
            ControlSignal::Return(value) => ControlSignal::Value(value),
            other => other,
        }
    }

    fn spec_impl_method_flow(
        &self,
        method_symbol: SymbolId,
        span: Span,
    ) -> Result<etas_hir::HirFlowDecl, ExecutionFault> {
        let Some(symbol) = self.checked.symbols.get(method_symbol) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "spec implementation method symbol is missing from checked symbols",
            ));
        };
        let SymbolDef::Item { item } = &symbol.def else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "spec implementation method symbol does not point at a HIR item",
            ));
        };
        let Some(HirItem::Impl(decl)) = self.checked.hir.items.get(*item) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "spec implementation method item is not an impl item",
            ));
        };
        decl.items
            .iter()
            .find_map(|item| match item {
                etas_hir::HirImplItem::Flow(flow) if flow.symbol == method_symbol => {
                    Some(flow.clone())
                }
                _ => None,
            })
            .ok_or_else(|| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "spec implementation method is missing from its checked impl item",
                )
            })
    }
}

fn implementation_types_match(
    store: &etas_types::TypeStore,
    implementation_self: etas_types::TypeId,
    receiver: etas_types::TypeId,
    left: &[etas_types::TypeId],
    right: &[etas_types::TypeId],
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut unifier = etas_types::TypeUnifier::new(store);
    unifier.unify(implementation_self, receiver).is_ok()
        && left
            .iter()
            .copied()
            .zip(right.iter().copied())
            .all(|(left, right)| unifier.unify(left, right).is_ok())
}

#[cfg(test)]
mod tests {
    use super::implementation_types_match;
    use etas_types::{PrimitiveType, Type, TypeInterner, TypeVarId};

    #[test]
    fn implementation_receiver_and_spec_args_share_generic_bindings() {
        let mut types = TypeInterner::default();
        let parameter = types.intern(Type::Var(TypeVarId(0)));
        let generic_receiver = types.intern(Type::Array(parameter));
        let integer = types.primitive(PrimitiveType::I32);
        let string = types.primitive(PrimitiveType::String);
        let actual_receiver = types.intern(Type::Array(integer));

        assert!(implementation_types_match(
            types.store(),
            generic_receiver,
            actual_receiver,
            &[parameter],
            &[integer]
        ));
        assert!(!implementation_types_match(
            types.store(),
            generic_receiver,
            actual_receiver,
            &[parameter],
            &[string]
        ));
        assert!(!implementation_types_match(
            types.store(),
            generic_receiver,
            actual_receiver,
            &[parameter, parameter],
            &[integer, string]
        ));
        assert!(!implementation_types_match(
            types.store(),
            generic_receiver,
            actual_receiver,
            &[parameter],
            &[]
        ));
    }
}
