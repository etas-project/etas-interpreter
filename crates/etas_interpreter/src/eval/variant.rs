use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub(super) fn named_variant_symbol(
        &self,
        path: Option<&etas_hir::ResolvedPath>,
    ) -> Option<SymbolId> {
        let ResolveResult::Resolved(symbol) = path?.resolution else {
            return None;
        };
        matches!(
            self.checked.symbols.get(symbol)?.def,
            SymbolDef::EnumVariant { .. } | SymbolDef::ExternalEnumVariant { .. }
        )
        .then_some(symbol)
    }

    pub(super) fn eval_named_variant(
        &self,
        expr: HirExprId,
        symbol: SymbolId,
        values: Vec<(String, InterpValue)>,
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        let missing = || {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "named enum constructor has no checked field layout",
            )
        };
        let layout = self.plan.named_variants.get(expr).ok_or_else(missing)?;
        let fields = layout.reorder(symbol, values).map_err(|message| {
            ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
        })?;
        let result = self.materialized_expression_type(expr, bindings, span)?;
        self.construct_enum_value(symbol, result, fields, span)
    }

    pub(super) fn eval_variant_constructor(
        &self,
        symbol: SymbolId,
        fields: Vec<InterpValue>,
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        let Some(etas_types::SymbolTypeFact::Flow { signature }) =
            self.checked.types.symbol_types.get(&symbol)
        else {
            return Err(
                self.enum_fact_fault(span, "enum constructor has no checked callable signature")
            );
        };
        let result = self.materialized_type(signature.output, bindings, span)?;
        self.construct_enum_value(symbol, result, fields, span)
    }

    fn construct_enum_value(
        &self,
        symbol: SymbolId,
        result: etas_types::TypeId,
        fields: Vec<InterpValue>,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        let (owner, name, arity) = self.enum_descriptor(symbol, span)?;
        if self.enum_owner(result) != Some(owner) {
            return Err(self.enum_fact_fault(
                span,
                "enum constructor result differs from its checked owner identity",
            ));
        }
        if fields.len() != arity {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "enum constructor `{name}` expects {arity} arguments, got {}",
                    fields.len()
                ),
            ));
        }
        Ok(InterpValue::Nominal {
            ty: result,
            value: InterpValue::Variant {
                name: name.into(),
                fields: fields.into(),
            }
            .into(),
        })
    }

    fn enum_descriptor(
        &self,
        symbol: SymbolId,
        span: Span,
    ) -> Result<(etas_types::TypeId, String, usize), ExecutionFault> {
        if let Some(constructor) = self.plan.dispatch.enum_constructor(symbol) {
            let Some(etas_types::SymbolTypeFact::Flow { signature }) =
                self.checked.types.symbol_types.get(&symbol)
            else {
                return Err(self
                    .enum_fact_fault(span, "standard enum constructor has no checked signature"));
            };
            let owner = self.enum_owner(signature.output).ok_or_else(|| {
                self.enum_fact_fault(span, "standard enum constructor has no checked owner")
            })?;
            return Ok((owner, constructor.name.clone(), constructor.arity));
        }
        let Some(symbol_data) = self.checked.symbols.get(symbol) else {
            return Err(self.enum_fact_fault(
                span,
                "enum variant symbol is missing from checked symbol table",
            ));
        };
        let Some((owner, _, variant)) = self
            .checked
            .types
            .enum_constructor(&self.checked.type_store, symbol_data)
        else {
            return Err(
                self.enum_fact_fault(span, "enum variant constructor has no checked layout")
            );
        };
        Ok((owner, variant.name.clone(), variant.fields.len()))
    }

    fn enum_owner(&self, ty: etas_types::TypeId) -> Option<etas_types::TypeId> {
        match self.checked.type_store.get(ty)? {
            etas_types::Type::Enum(_) => Some(ty),
            etas_types::Type::Applied { constructor, .. }
                if matches!(
                    self.checked
                        .type_store
                        .get(etas_types::TypeId(constructor.0)),
                    Some(etas_types::Type::Enum(_))
                ) =>
            {
                Some(etas_types::TypeId(constructor.0))
            }
            _ => None,
        }
    }

    fn enum_fact_fault(&self, span: Span, message: impl Into<String>) -> ExecutionFault {
        ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
    }

    pub(super) fn eval_enum_reference(
        &self,
        expr: HirExprId,
        symbol: SymbolId,
        frame: &Frame,
        span: Span,
    ) -> Result<Option<InterpValue>, ExecutionFault> {
        if !(self.plan.dispatch.enum_constructor(symbol).is_some()
            || self
                .checked
                .symbols
                .get(symbol)
                .is_some_and(|symbol| symbol.kind == SymbolKind::EnumVariant))
            || frame.get(symbol).is_some()
        {
            return Ok(None);
        }
        let (owner, _, _) = self.enum_descriptor(symbol, span)?;
        let ty = *self
            .checked
            .types
            .expr_types
            .get(&expr)
            .ok_or_else(|| self.enum_fact_fault(span, "enum reference has no checked type"))?;
        let output = match self.checked.type_store.get(ty) {
            Some(etas_types::Type::Function(signature)) => {
                self.materialized_type(signature.output, frame.generic_bindings(), span)?
            }
            _ => {
                return self
                    .construct_enum_value(
                        symbol,
                        self.materialized_type(ty, frame.generic_bindings(), span)?,
                        Vec::new(),
                        span,
                    )
                    .map(Some);
            }
        };
        if self.enum_owner(output) != Some(owner) {
            return Err(self.enum_fact_fault(
                span,
                "enum callable reference has a different checked owner",
            ));
        }
        let layout = self.checked.types.enum_layouts.get(&owner).ok_or_else(|| {
            self.enum_fact_fault(
                span,
                "enum callable reference has no checked generic layout",
            )
        })?;
        let args = match self.checked.type_store.get(output) {
            Some(etas_types::Type::Applied { args, .. }) => args.as_slice(),
            _ => &[],
        };
        if args.len() != layout.type_params.len() {
            return Err(self.enum_fact_fault(
                span,
                "enum callable reference has an inconsistent generic arity",
            ));
        }
        let target = CallTarget::EnumVariant(symbol);
        let target = if args.is_empty() {
            target
        } else {
            CallTarget::Specialized {
                target: target.into(),
                type_bindings: layout
                    .type_params
                    .iter()
                    .cloned()
                    .zip(args.iter().copied())
                    .collect(),
                effect_row_bindings: Vec::new(),
            }
        };
        Ok(Some(InterpValue::Callable(target)))
    }

    pub(super) fn resume_variant_args(
        &mut self,
        variant_symbol: SymbolId,
        args: Vec<HirArg>,
        start_arg_index: usize,
        mut evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        for (index, arg) in args.iter().enumerate().skip(start_arg_index) {
            let expr = match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            };
            match self.eval_expr(expr, frame) {
                ControlSignal::Value(value) => evaluated_args.push(value),
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
                | ControlSignal::Host(_)) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::VariantArgs {
                            variant_symbol,
                            args: args.clone(),
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame: frame.clone(),
                        },
                    );
                }
                ControlSignal::Return(value) => return ControlSignal::Return(value),
                ControlSignal::Resume(value) => return ControlSignal::Resume(value),
                ControlSignal::Finish(value) => return ControlSignal::Finish(value),
                ControlSignal::Break => return ControlSignal::Break,
                ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
                ControlSignal::Cancelled(cause) => return ControlSignal::Cancelled(cause),
                ControlSignal::Continue => return ControlSignal::Continue,
            }
        }
        match self.eval_variant_constructor(
            variant_symbol,
            evaluated_args,
            frame.generic_bindings(),
            span,
        ) {
            Ok(value) => ControlSignal::Value(value),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }
}
