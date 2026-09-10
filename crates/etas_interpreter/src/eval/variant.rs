use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_variant_constructor(
        &self,
        symbol: SymbolId,
        fields: Vec<InterpValue>,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        if let Some(constructor) = self.plan.dispatch.enum_constructor(symbol) {
            if fields.len() != constructor.arity {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!(
                        "enum constructor `{}` expects {} arguments, got {}",
                        constructor.name,
                        constructor.arity,
                        fields.len()
                    ),
                ));
            }
            return Ok(InterpValue::Variant {
                name: constructor.name.clone(),
                fields,
            });
        }
        let Some(symbol_data) = self.checked.symbols.get(symbol) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "enum variant symbol is missing from checked symbol table",
            ));
        };
        let SymbolDef::EnumVariant {
            enum_item,
            variant_index,
        } = symbol_data.def
        else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "enum variant constructor does not point at a variant definition",
            ));
        };
        let Some(HirItem::Enum(enum_decl)) = self.checked.hir.items.get(enum_item) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "enum variant constructor target item is missing",
            ));
        };
        let Some(variant) = enum_decl.variants.get(variant_index as usize) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "enum variant constructor index is out of bounds",
            ));
        };
        let expected_arity = variant.fields.len();
        if fields.len() != expected_arity {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "variant constructor `{}` expects {} arguments, got {}",
                    symbol_data.name,
                    expected_arity,
                    fields.len()
                ),
            ));
        }
        match (symbol_data.name.as_str(), fields.as_slice()) {
            ("None", []) => Ok(InterpValue::OptionNone),
            ("Some", [value]) => Ok(InterpValue::OptionSome(Box::new(value.clone()))),
            _ => Ok(InterpValue::Variant {
                name: symbol_data.name.clone(),
                fields,
            }),
        }
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
        match self.eval_variant_constructor(variant_symbol, evaluated_args, span) {
            Ok(value) => ControlSignal::Value(value),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }
}
