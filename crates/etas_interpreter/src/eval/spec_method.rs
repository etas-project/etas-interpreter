use super::*;
use crate::control::ExecutionFault;

mod generics;

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
            frame,
            span,
        ) {
            Ok(target) => target,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let target = match self.specialize_spec_method_target(
            expr,
            target,
            spec_symbol,
            method,
            frame,
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
        frame: &Frame,
        span: Span,
    ) -> Result<CallTarget, ExecutionFault> {
        let spec_symbol = self
            .checked
            .hir
            .canonical_source_symbol(spec_symbol)
            .ok_or_else(|| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "spec method is missing its canonical checked spec symbol",
                )
            })?;
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
        let receiver_ty = self.materialized_type(receiver_ty, frame.generic_bindings(), span)?;
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
        let spec_arg_tys = self
            .checked_spec_arg_types(spec_args, span)?
            .into_iter()
            .map(|ty| self.materialized_type(ty, frame.generic_bindings(), span))
            .collect::<Result<Vec<_>, _>>()?;
        let mut matches = self
            .checked
            .types
            .spec_impls
            .iter()
            .filter(|implementation| {
                implementation.spec_symbol == spec_symbol
                    && self.spec_type_ids_match(implementation.self_type, receiver_ty)
                    && type_arg_lists_match(
                        &self.checked.type_store,
                        &implementation.args,
                        &spec_arg_tys,
                    )
            })
            .filter_map(|implementation| {
                implementation
                    .methods
                    .iter()
                    .find(|candidate| candidate.name == method)
                    .map(|candidate| candidate.symbol)
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|symbol| symbol.0);
        matches.dedup();
        match matches.as_slice() {
            [symbol] => {
                let Some(SymbolDef::Item { item }) =
                    self.checked.symbols.get(*symbol).map(|symbol| &symbol.def)
                else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "spec method has no checked executable item",
                    ));
                };
                if !matches!(self.checked.hir.items.get(*item), Some(HirItem::Flow(_))) {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "spec method target is not a checked flow item",
                    ));
                }
                Ok(CallTarget::FlowItem(*item))
            }
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

    fn checked_spec_arg_types(
        &self,
        spec_args: &[HirTypeId],
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
            lowered.push(ty);
        }
        Ok(lowered)
    }

    fn spec_type_ids_match(&self, left: etas_types::TypeId, right: etas_types::TypeId) -> bool {
        if left == right {
            return true;
        }
        let mut unifier = etas_types::TypeUnifier::new(&self.checked.type_store);
        unifier.unify(left, right).is_ok()
    }
}

fn type_arg_lists_match(
    store: &etas_types::TypeStore,
    left: &[etas_types::TypeId],
    right: &[etas_types::TypeId],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .copied()
            .zip(right.iter().copied())
            .all(|(left, right)| {
                if left == right {
                    return true;
                }
                let mut unifier = etas_types::TypeUnifier::new(store);
                unifier.unify(left, right).is_ok()
            })
}
