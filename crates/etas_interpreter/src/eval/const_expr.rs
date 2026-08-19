use std::collections::HashSet;

use super::*;
use crate::control::ExecutionFault;
use crate::value::{RangeBounds, RangeValue};

impl<'a> EvalContext<'a> {
    pub(super) fn eval_literal(
        &self,
        expr: HirExprId,
        literal: &HirLiteral,
    ) -> Result<InterpValue, ExecutionFault> {
        match literal {
            HirLiteral::Bool { value, .. } => Ok(InterpValue::Bool(*value)),
            HirLiteral::Int { text, span } => {
                let primitive = self.numeric_literal_type(expr, false, *span)?;
                crate::value::NumericValue::parse_integer(text, primitive)
                    .map(InterpValue::Number)
                    .map_err(|_| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            *span,
                            format!(
                                "integer literal `{text}` cannot be represented as {} at runtime",
                                primitive.source_name()
                            ),
                        )
                    })
            }
            HirLiteral::String { value, .. } => Ok(InterpValue::String(value.clone())),
            HirLiteral::Char { value, .. } => Ok(InterpValue::String(value.to_string())),
            HirLiteral::Float { text, span } => {
                let primitive = self.numeric_literal_type(expr, true, *span)?;
                crate::value::NumericValue::parse_float(text, primitive)
                    .map(InterpValue::Number)
                    .map_err(|_| ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        *span,
                        format!(
                            "floating-point literal `{text}` cannot be represented as {} at runtime",
                            primitive.source_name()
                        ),
                    ))
            }
        }
    }

    pub(super) fn numeric_literal_type(
        &self,
        expr: HirExprId,
        is_float: bool,
        span: Span,
    ) -> Result<etas_types::PrimitiveType, ExecutionFault> {
        let Some(ty) = self.checked.types.expr_types.get(&expr).copied() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "numeric literal is missing its checked type",
            ));
        };
        match self.checked.type_store.get(ty) {
            Some(etas_types::Type::IntegerLiteral { .. }) if !is_float => {
                Ok(etas_types::PrimitiveType::I32)
            }
            Some(etas_types::Type::Primitive(primitive))
                if matches!(
                    primitive,
                    etas_types::PrimitiveType::I8
                        | etas_types::PrimitiveType::I16
                        | etas_types::PrimitiveType::I32
                        | etas_types::PrimitiveType::I64
                        | etas_types::PrimitiveType::I128
                        | etas_types::PrimitiveType::ISize
                        | etas_types::PrimitiveType::U8
                        | etas_types::PrimitiveType::U16
                        | etas_types::PrimitiveType::U32
                        | etas_types::PrimitiveType::U64
                        | etas_types::PrimitiveType::U128
                        | etas_types::PrimitiveType::USize
                ) && !is_float =>
            {
                Ok(*primitive)
            }
            Some(etas_types::Type::Primitive(
                primitive @ (etas_types::PrimitiveType::F32 | etas_types::PrimitiveType::F64),
            )) if is_float => Ok(*primitive),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "numeric literal checked type does not match its literal kind",
            )),
        }
    }

    pub(super) fn eval_path_signal(
        &mut self,
        resolution: ResolveResult,
        span: Span,
        frame: &Frame,
    ) -> ControlSignal {
        match resolution {
            ResolveResult::Resolved(symbol) => {
                if let Some(value) = frame.get(symbol) {
                    return ControlSignal::Value(value);
                }
                match self.eval_global_symbol(symbol, span) {
                    Ok(value) => ControlSignal::Value(value),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                }
            }
            ResolveResult::PartiallyResolved(partial)
                if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking =>
            {
                let Some(prefix) = partial.resolved_prefix else {
                    return ControlSignal::missing_checked_fact(
                        "partial path is missing its resolved prefix symbol",
                        span,
                    );
                };
                let mut value = if let Some(value) = frame.get(prefix) {
                    value
                } else {
                    match self.eval_global_symbol(prefix, span) {
                        Ok(value) => value,
                        Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                    }
                };
                for field in partial.remaining {
                    match self.eval_field_value_signal(None, value, &field, span) {
                        ControlSignal::Value(next) => value = next,
                        other => return other,
                    }
                }
                ControlSignal::Value(value)
            }
            _ => ControlSignal::missing_checked_fact("path was not fully resolved", span),
        }
    }

    fn eval_global_symbol(
        &mut self,
        symbol: SymbolId,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        let mut visiting = HashSet::new();
        self.eval_global_symbol_with_cycle_check(symbol, span, &mut visiting)
    }

    fn eval_global_symbol_with_cycle_check(
        &mut self,
        symbol: SymbolId,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ExecutionFault> {
        if !visiting.insert(symbol) {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                "cyclic top-level `let` evaluation reached interpreter execution",
            ));
        }
        let value = (|| {
            let Some(symbol_data) = self.checked.symbols.get(symbol).cloned() else {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "resolved global symbol is missing from checked symbol table",
                ));
            };
            match &symbol_data.def {
                SymbolDef::TopLevelLet {
                    initializer,
                    classification,
                    ..
                } => match classification {
                    TopLevelLetClassification::Const => {
                        self.eval_const_expr(*initializer, symbol_data.definition_span, visiting)
                    }
                    TopLevelLetClassification::Handler => {
                        self.eval_top_level_handler(*initializer, symbol_data.definition_span)
                    }
                    TopLevelLetClassification::ResourceHandle(_) => self.resource_handle_value(
                        symbol,
                        symbol_data.name.clone(),
                        *initializer,
                        span,
                    ),
                    TopLevelLetClassification::Unknown | TopLevelLetClassification::Invalid => {
                        Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "top-level `let` is missing a valid constant, handler, or resource-handle classification",
                        ))
                    }
                },
                SymbolDef::EnumVariant { .. } => {
                    self.eval_variant_constructor(symbol, Vec::new(), span)
                }
                SymbolDef::ImportAlias { path, .. } => {
                    if let Some(value) = self.eval_std_value_alias(path) {
                        Ok(value)
                    } else if let Some(item) = self
                        .source_item_for_import_path(path)
                        .and_then(|ast_item| self.hir_item_for_ast_item(&ast_item))
                        .and_then(|item| self.checked.hir.items.get(item))
                        && let HirItem::TopLevelLet(item) = item
                    {
                        self.eval_global_symbol_with_cycle_check(item.symbol, span, visiting)
                    } else {
                        Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            "import alias does not name a runtime value",
                        ))
                    }
                }
                _ => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "path does not name a runtime value in the current frame",
                )),
            }
        })();
        visiting.remove(&symbol);
        value
    }

    fn eval_std_value_alias(&self, path: &[String]) -> Option<InterpValue> {
        if matches!(path, [std, option, none] if std == "std" && option == "option" && none == "None")
        {
            return Some(InterpValue::OptionNone);
        }
        let symbol = self.checked.std_registry.lookup_qualified(path)?;
        let etas_std::StdDecl::Value(value) = &symbol.decl else {
            return None;
        };
        Some(InterpValue::Variant {
            name: value.name.clone(),
            fields: Vec::new(),
        })
    }

    fn eval_top_level_handler(
        &mut self,
        expr: HirExprId,
        definition_span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        let Some(HirExpr::Handler { handlers, span }) = self.checked.hir.exprs.get(expr) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                definition_span,
                "top-level handler classification points at a non-handler initializer",
            ));
        };
        match self.eval_handler_value_expr(expr, handlers, *span) {
            ControlSignal::Value(value) => Ok(value),
            ControlSignal::Fault(fault) => Err(*fault),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                definition_span,
                "top-level handler initializer produced a non-value control signal",
            )),
        }
    }

    fn eval_const_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ExecutionFault> {
        let Some(expr_data) = self.checked.hir.exprs.get(expr).cloned() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant expression is missing from checked HIR",
            ));
        };
        match &expr_data {
            HirExpr::Literal(literal) => self.eval_literal(expr, literal),
            HirExpr::Path(path) => {
                let ResolveResult::Resolved(symbol) = path.resolution else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "constant path initializer was not fully resolved",
                    ));
                };
                self.eval_global_symbol_with_cycle_check(symbol, path.span, visiting)
            }
            HirExpr::Tuple { elems, .. } => Ok(InterpValue::Tuple(
                elems
                    .iter()
                    .map(|expr| self.eval_const_expr(*expr, span, visiting))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            HirExpr::Array { elems, .. } => Ok(InterpValue::Array(ArrayValue::new(
                elems
                    .iter()
                    .map(|expr| self.eval_const_expr(*expr, span, visiting))
                    .collect::<Result<Vec<_>, _>>()?,
            ))),
            HirExpr::List { elems, .. } => Ok(InterpValue::List(
                elems
                    .iter()
                    .map(|expr| self.eval_const_expr(*expr, span, visiting))
                    .collect::<Result<Vec<_>, _>>()?
                    .into(),
            )),
            HirExpr::ListCons { head, tail, span } => {
                self.eval_const_list_cons(*head, *tail, *span, visiting)
            }
            HirExpr::EmptySequence { span } => self.eval_const_empty_sequence(expr, *span),
            HirExpr::Set { elems, .. } => Ok(InterpValue::Set(
                elems
                    .iter()
                    .map(|expr| self.eval_const_expr(*expr, span, visiting))
                    .collect::<Result<Vec<_>, _>>()?
                    .into(),
            )),
            HirExpr::Map { entries, .. } => {
                let mut values = Vec::with_capacity(entries.len());
                for entry in entries {
                    values.push((
                        self.eval_const_expr(entry.key, span, visiting)?,
                        self.eval_const_expr(entry.value, span, visiting)?,
                    ));
                }
                Ok(InterpValue::Map(MapValue::new(values)))
            }
            HirExpr::Range {
                start, end, bounds, ..
            } => Ok(InterpValue::Range(RangeValue {
                start: Box::new(self.eval_const_expr(*start, span, visiting)?),
                end: Box::new(self.eval_const_expr(*end, span, visiting)?),
                bounds: match bounds {
                    etas_hir::HirRangeBounds::ClosedOpen => RangeBounds::ClosedOpen,
                    etas_hir::HirRangeBounds::OpenClosed => RangeBounds::OpenClosed,
                },
            })),
            HirExpr::EmptyRecordOrMap { span } => self.eval_const_empty_record_or_map(expr, *span),
            HirExpr::Record(record) => self.eval_const_record_expr(expr, record, span, visiting),
            HirExpr::Unary {
                op,
                expr: inner,
                span,
            } => self.eval_const_unary_expr(*op, *inner, *span, visiting),
            HirExpr::Binary { op, lhs, rhs, span } => {
                self.eval_const_binary_expr(*op, *lhs, *rhs, *span, visiting)
            }
            HirExpr::Field { base, field, span } => {
                let base = self.eval_const_expr(*base, *span, visiting)?;
                self.eval_const_field_value(Some(expr), base, field, *span)
            }
            HirExpr::Index { base, index, span } => {
                let base = self.eval_const_expr(*base, *span, visiting)?;
                let index = self.eval_const_expr(*index, *span, visiting)?;
                match self.eval_index_value(expr, base, index, *span) {
                    ControlSignal::Value(value) => Ok(value),
                    ControlSignal::Fault(fault) => Err(*fault),
                    _ => Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        *span,
                        "fallible checked index cannot be evaluated as a constant expression",
                    )),
                }
            }
            HirExpr::Slice {
                base,
                start,
                end,
                bounds,
                span,
            } => {
                let base = self.eval_const_expr(*base, *span, visiting)?;
                let start = self.eval_const_expr(*start, *span, visiting)?;
                let end = self.eval_const_expr(*end, *span, visiting)?;
                self.eval_slice_value(expr, base, start, end, *bounds, *span)
            }
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "constant evaluation requires a pure local constant expression",
            )),
        }
    }

    fn eval_const_record_expr(
        &mut self,
        expr: HirExprId,
        record: &etas_hir::HirRecordExpr,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ExecutionFault> {
        let mut fields = Vec::with_capacity(record.fields.len());
        for field in &record.fields {
            match field {
                etas_hir::HirFieldInit::Shorthand {
                    name,
                    resolution,
                    span,
                } => {
                    let value = match resolution {
                        ResolveResult::Resolved(symbol) => {
                            self.eval_global_symbol_with_cycle_check(*symbol, *span, visiting)?
                        }
                        _ => {
                            return Err(ExecutionFault::new(
                                AnalysisDiagnosticCode::MissingCheckedFact,
                                *span,
                                "constant record shorthand field was not fully resolved",
                            ));
                        }
                    };
                    fields.push((name.clone(), value));
                }
                etas_hir::HirFieldInit::Named { name, value, .. } => {
                    fields.push((name.clone(), self.eval_const_expr(*value, span, visiting)?));
                }
            }
        }
        let representation = InterpValue::Record(fields.into());
        if record.path.is_none() {
            return Ok(representation);
        }
        let Some(ty) = self.checked.types.expr_types.get(&expr).copied() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant nominal record constructor is missing its checked result type",
            ));
        };
        Ok(InterpValue::Nominal {
            ty,
            value: Box::new(representation),
        })
    }

    fn eval_const_empty_record_or_map(
        &self,
        expr: HirExprId,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        if !self.checked.types.expr_types.contains_key(&expr) {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant empty brace literal is missing its checked type",
            ));
        };
        match self.plan.dispatch.brace_literal_shape(expr) {
            Some(BraceLiteralShape::Record) => Ok(InterpValue::Record(
                Vec::<(String, InterpValue)>::new().into(),
            )),
            Some(BraceLiteralShape::Map) => Ok(InterpValue::Map(MapValue::new(Vec::new()))),
            None => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant empty brace literal must be checked as a record or Map[K, V]",
            )),
        }
    }

    fn eval_const_list_cons(
        &mut self,
        head: HirExprId,
        tail: HirExprId,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ExecutionFault> {
        let head = self.eval_const_expr(head, span, visiting)?;
        let tail = self.eval_const_expr(tail, span, visiting)?;
        let InterpValue::List(values) = tail else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant list cons tail must evaluate to a List[T]",
            ));
        };
        let mut result = values.snapshot();
        result.insert(0, head);
        Ok(InterpValue::List(result.into()))
    }

    fn eval_const_empty_sequence(
        &self,
        expr: HirExprId,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        let Some(ty) = self.checked.types.expr_types.get(&expr) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant empty sequence is missing its checked type",
            ));
        };
        match self.checked.type_store.get(*ty) {
            Some(etas_types::Type::Array(_)) => Ok(InterpValue::Array(ArrayValue::new(Vec::new()))),
            Some(etas_types::Type::List(_)) => Ok(InterpValue::List(Vec::new().into())),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant empty sequence must be checked as Array[T] or List[T]",
            )),
        }
    }

    fn eval_const_unary_expr(
        &mut self,
        op: HirUnaryOp,
        expr: HirExprId,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ExecutionFault> {
        let value = self.eval_const_expr(expr, span, visiting)?;
        self.eval_unary_value(op, value, span)
    }

    fn eval_const_binary_expr(
        &mut self,
        op: HirBinaryOp,
        lhs: HirExprId,
        rhs: HirExprId,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ExecutionFault> {
        let left = self.eval_const_expr(lhs, span, visiting)?;
        match op {
            HirBinaryOp::AndAnd => {
                let left_bool = self.expect_bool(left, span)?;
                if !left_bool {
                    return Ok(InterpValue::Bool(false));
                }
                let right = self.eval_const_expr(rhs, span, visiting)?;
                let right_bool = self.expect_bool(right, span)?;
                return Ok(InterpValue::Bool(right_bool));
            }
            HirBinaryOp::OrOr => {
                let left_bool = self.expect_bool(left, span)?;
                if left_bool {
                    return Ok(InterpValue::Bool(true));
                }
                let right = self.eval_const_expr(rhs, span, visiting)?;
                let right_bool = self.expect_bool(right, span)?;
                return Ok(InterpValue::Bool(right_bool));
            }
            _ => {}
        }
        let right = self.eval_const_expr(rhs, span, visiting)?;
        self.eval_binary_values(op, left, right, span)
    }

    fn resource_handle_value(
        &self,
        symbol: SymbolId,
        name: String,
        _initializer: HirExprId,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        let ty = self.resource_handle_type(symbol, span)?;
        let stable_id = self.resource_handle_stable_id(symbol, span)?;
        Ok(InterpValue::ResourceHandle {
            name,
            stable_id,
            ty,
        })
    }

    fn resource_handle_type(
        &self,
        symbol: SymbolId,
        span: Span,
    ) -> Result<etas_types::TypeId, ExecutionFault> {
        match self.checked.types.symbol_types.get(&symbol) {
            Some(etas_types::SymbolTypeFact::TopLevelLet { ty, .. }) => Ok(*ty),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "resource handle is missing its checked type",
            )),
        }
    }

    fn resource_handle_stable_id(
        &self,
        symbol: SymbolId,
        span: Span,
    ) -> Result<String, ExecutionFault> {
        match self.checked.types.resource_handles.get(&symbol) {
            Some(etas_types::ResourceHandleFact::MemoryRegion { stable_id, .. }) => {
                Ok(stable_id.clone())
            }
            None => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "memory-region resource handle is missing checked resource handle facts",
            )),
        }
    }
}
