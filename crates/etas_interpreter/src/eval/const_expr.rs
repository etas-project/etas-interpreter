use std::collections::HashSet;

use super::*;
use crate::control::ExecutionFault;
use crate::value::{RangeBounds, RangeValue};

mod failure;
mod source_literal;
use failure::ConstantFailure;

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
            HirLiteral::String { value, .. } => Ok(InterpValue::String(value.clone().into())),
            HirLiteral::Char { value, .. } => Ok(InterpValue::String(value.to_string().into())),
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
        let Some(ty) = self.checked.types.expression_producer_type(expr) else {
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
                if frame.receiver_is_held(symbol) {
                    return ControlSignal::invalid_arguments(
                        "receiver binding is exclusively held by an updating call",
                        span,
                    );
                }
                if let Some(value) = frame.get(symbol) {
                    return ControlSignal::Value(value);
                }
                match self.eval_global_symbol(symbol, span) {
                    Ok(value) => ControlSignal::Value(value),
                    Err(error) => error.into_signal(),
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
                        Err(error) => return error.into_signal(),
                    }
                };
                for (index, field) in partial.remaining.iter().enumerate() {
                    let site = crate::plan::FieldAccessSite::Path {
                        prefix,
                        remaining: &partial.remaining,
                        index,
                    };
                    match self.eval_field_value_signal(site, value, field, span) {
                        ControlSignal::Value(next) => value = next,
                        other => return other,
                    }
                }
                ControlSignal::Value(value)
            }
            _ => ControlSignal::missing_checked_fact("path was not fully resolved", span),
        }
    }

    pub(super) fn eval_global_symbol(
        &mut self,
        symbol: SymbolId,
        span: Span,
    ) -> Result<InterpValue, ConstantFailure> {
        let mut visiting = HashSet::new();
        self.eval_global_symbol_with_cycle_check(symbol, span, &mut visiting)
    }

    fn eval_global_symbol_with_cycle_check(
        &mut self,
        symbol: SymbolId,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ConstantFailure> {
        if !visiting.insert(symbol) {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                "cyclic top-level `let` evaluation reached interpreter execution",
            )
            .into());
        }
        let value = (|| {
            let Some(symbol_data) = self.checked.symbols.get(symbol).cloned() else {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "resolved global symbol is missing from checked symbol table",
                )
                .into());
            };
            match &symbol_data.def {
                SymbolDef::Item { item }
                    if self.checked.types.item_signatures.contains_key(item) =>
                {
                    match self.checked.hir.items.get(*item) {
                        Some(HirItem::Agent(_)) => self
                            .source_agent_call_target(*item, span)
                            .map(InterpValue::Callable)
                            .map_err(Into::into),
                        Some(HirItem::Flow(_)) => self
                            .source_flow_call_target(*item, span)
                            .map(InterpValue::Callable)
                            .map_err(Into::into),
                        Some(HirItem::Tool(_)) => self
                            .source_tool_call_target(*item, span)
                            .map(InterpValue::Callable)
                            .map_err(Into::into),
                        _ => Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            "checked declaration is not a runtime callable value",
                        ).into()),
                    }
                }
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
                            .map_err(Into::into)
                    }
                    TopLevelLetClassification::ResourceHandle(_) => self.resource_handle_value(
                        symbol,
                        symbol_data.name.clone(),
                        *initializer,
                        span,
                    ).map_err(Into::into),
                    TopLevelLetClassification::Unknown | TopLevelLetClassification::Invalid => {
                        Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "top-level `let` is missing a valid constant, handler, or resource-handle classification",
                        ).into())
                    }
                },
                SymbolDef::EnumVariant { .. } | SymbolDef::ExternalEnumVariant { .. } => self
                    .eval_variant_constructor(
                        symbol,
                        Vec::new(),
                        &crate::control::GenericBindings::new(),
                        span,
                    ).map_err(Into::into),
                SymbolDef::ImportAlias { path, .. } => {
                    if let Some(target) = self.artifact_std_callable(symbol, span)? {
                        Ok(InterpValue::Callable(target))
                    } else if let Some(constructor) = self.plan.dispatch.enum_constructor(symbol) {
                        if constructor.arity == 0 {
                            self.eval_variant_constructor(
                                symbol,
                                Vec::new(),
                                &crate::control::GenericBindings::new(),
                                span,
                            ).map_err(Into::into)
                        } else {
                            Ok(InterpValue::Callable(CallTarget::EnumVariant(symbol)))
                        }
                    } else if let Some(value) = self.eval_artifact_std_value(path, span)? {
                        Ok(value)
                    } else if let Some(value) = self.eval_std_value_alias(symbol, path, span)? {
                        Ok(value)
                    } else if let Some(item) = self.source_item_id_for_import_path(path) {
                        match self.checked.hir.items.get(item) {
                            Some(HirItem::TopLevelLet(item)) => self
                                .eval_global_symbol_with_cycle_check(item.symbol, span, visiting),
                            Some(HirItem::Agent(agent)) => self
                                .eval_global_symbol_with_cycle_check(agent.symbol, span, visiting),
                            Some(HirItem::Flow(flow)) => self.eval_global_symbol_with_cycle_check(
                                flow.symbol,
                                span,
                                visiting,
                            ),
                            _ => Err(ExecutionFault::new(
                                AnalysisDiagnosticCode::InvalidArguments,
                                span,
                                "import alias does not name a runtime value",
                            ).into()),
                        }
                    } else {
                        Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            "import alias does not name a runtime value",
                        ).into())
                    }
                }
                _ => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "path does not name a runtime value in the current frame",
                ).into()),
            }
        })();
        visiting.remove(&symbol);
        value
    }

    fn eval_artifact_std_value(
        &mut self,
        path: &[String],
        span: Span,
    ) -> Result<Option<InterpValue>, ConstantFailure> {
        let Some(definition) = self
            .checked
            .std_registry
            .lookup_qualified(path)
            .filter(|declaration| matches!(declaration.decl, etas_std::StdDecl::Value(_)))
            .and_then(|declaration| self.checked.std_registry.declaration_key(declaration.id))
            .cloned()
        else {
            return Ok(None);
        };
        let missing =
            |reason| ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason);
        let library = self
            .plan
            .standard_library
            .as_ref()
            .cloned()
            .ok_or_else(|| {
                missing("source std value has no checked library execution plan".to_owned())
            })?;
        let (symbol, ty, program) = library.value_program(&definition).map_err(&missing)?;
        let value = self
            .scoped_context(&program, self.handler_floor)
            .eval_global_symbol(symbol, span)?;
        let scope = etas_code::descriptor::GenericOwner::Declaration(definition);
        library
            .bridge()
            .value_to_original_scoped(&program.checked, &scope, ty, value, &program)
            .map(Some)
            .map_err(|error| missing(error.to_string()).into())
    }

    fn eval_std_value_alias(
        &self,
        symbol: SymbolId,
        path: &[String],
        span: Span,
    ) -> Result<Option<InterpValue>, ExecutionFault> {
        let Some(declaration) = self.checked.std_registry.lookup_qualified(path) else {
            return Ok(None);
        };
        if declaration
            .intrinsic
            .as_ref()
            .is_some_and(|descriptor| descriptor.id.0 == etas_std::intrinsic::pure::OPTION_NONE)
        {
            return Ok(Some(InterpValue::OptionNone));
        }
        let etas_std::StdDecl::Value(value) = &declaration.decl else {
            return Ok(None);
        };
        let Some(etas_types::SymbolTypeFact::Value { ty }) =
            self.checked.types.symbol_types.get(&symbol)
        else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "standard value has no checked type identity",
            ));
        };
        let mut layouts = crate::plan::code::CheckedValueTypes::default();
        if let Some(role) = value.runtime_role {
            let value = crate::value::StdConstantValue::checked(
                role,
                *ty,
                &self.checked.types.known_std_types,
            )
            .map_err(|reason| {
                ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason)
            })?;
            return Ok(Some(InterpValue::StdConstant(value)));
        }
        let variant = layouts
            .variant(self.checked, *ty, &value.name)
            .map_err(|error| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!("standard value has no checked constructor layout: {error:?}"),
                )
            })?;
        if !variant.fields.is_empty() {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "standard singleton value has a nonempty payload",
            ));
        }
        Ok(Some(InterpValue::Nominal {
            ty: *ty,
            value: InterpValue::Variant {
                name: value.name.clone().into(),
                fields: Vec::new().into(),
            }
            .into(),
        }))
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
        match self.eval_handler_value_expr(expr, handlers, *span, None) {
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
    ) -> Result<InterpValue, ConstantFailure> {
        let Some(expr_data) = self.checked.hir.exprs.get(expr).cloned() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant expression is missing from checked HIR",
            )
            .into());
        };
        match &expr_data {
            HirExpr::MethodCall { .. }
                if self.checked.hir.source_map.is_collection_literal(expr) =>
            {
                self.eval_const_source_literal(expr, span)
            }
            HirExpr::Literal(literal) => self.eval_literal(expr, literal).map_err(Into::into),
            HirExpr::Lambda { .. } => self
                .capture_lambda_value(expr, span, None)
                .map_err(Into::into),
            HirExpr::Path(path) => {
                let ResolveResult::Resolved(symbol) = path.resolution else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "constant path initializer was not fully resolved",
                    )
                    .into());
                };
                let frame = Frame::new(std::sync::Arc::default());
                if let Some(value) = self.eval_enum_reference(expr, symbol, &frame, path.span)? {
                    return Ok(value);
                }
                if self.std_intrinsic(symbol).is_none()
                    && !self
                        .checked
                        .types
                        .generic_instantiations
                        .contains_key(&expr)
                {
                    return self.eval_global_symbol_with_cycle_check(symbol, path.span, visiting);
                }
                let frame = Frame::new(std::sync::Arc::default());
                let value = match self.eval_std_callable_value(expr, &frame, path.span)? {
                    Some(value) => value,
                    None => {
                        self.eval_global_symbol_with_cycle_check(symbol, path.span, visiting)?
                    }
                };
                match value {
                    InterpValue::Callable(target) => self
                        .specialize_call_target_for_call(expr, target, &frame, path.span)
                        .map(InterpValue::Callable)
                        .map_err(Into::into),
                    value => Ok(value),
                }
            }
            HirExpr::Tuple { elems, .. } if elems.is_empty() => Ok(InterpValue::Unit),
            HirExpr::Tuple { elems, .. } => Ok(InterpValue::Tuple(
                elems
                    .iter()
                    .map(|expr| self.eval_const_expr(*expr, span, visiting))
                    .collect::<Result<Vec<_>, _>>()?
                    .into(),
            )),
            HirExpr::Array { elems, .. } => Ok(InterpValue::Array(ArrayValue::new(
                elems
                    .iter()
                    .map(|expr| self.eval_const_expr(*expr, span, visiting))
                    .collect::<Result<Vec<_>, _>>()?,
            ))),
            HirExpr::List { elems, .. } => {
                let values = elems
                    .iter()
                    .map(|expr| self.eval_const_expr(*expr, span, visiting))
                    .collect::<Result<Vec<_>, _>>()?
                    .into();
                self.checked_sequence_value(expr, values, None, span)
                    .map_err(Into::into)
            }
            HirExpr::ListCons { head, tail, span } => {
                self.eval_const_list_cons(*head, *tail, *span, visiting)
            }
            HirExpr::EmptySequence { span } => self
                .eval_const_empty_sequence(expr, *span)
                .map_err(Into::into),
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
            HirExpr::EmptyRecordOrMap { span } => self
                .eval_const_empty_record_or_map(expr, *span)
                .map_err(Into::into),
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
                self.eval_const_field_value(
                    crate::plan::FieldAccessSite::Expr(expr),
                    base,
                    field,
                    *span,
                )
                .map_err(Into::into)
            }
            HirExpr::Index { base, index, span } => {
                let base = self.eval_const_expr(*base, *span, visiting)?;
                let index = self.eval_const_expr(*index, *span, visiting)?;
                match self.eval_index_value(expr, base, index, *span) {
                    ControlSignal::Value(value) => Ok(value),
                    ControlSignal::Fault(fault) => Err((*fault).into()),
                    _ => Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        *span,
                        "fallible checked index cannot be evaluated as a constant expression",
                    )
                    .into()),
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
                    .map_err(Into::into)
            }
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "constant evaluation requires a pure local constant expression",
            )
            .into()),
        }
    }

    fn eval_const_record_expr(
        &mut self,
        expr: HirExprId,
        record: &etas_hir::HirRecordExpr,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ConstantFailure> {
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
                            )
                            .into());
                        }
                    };
                    fields.push((name.clone(), value));
                }
                etas_hir::HirFieldInit::Named { name, value, .. } => {
                    fields.push((name.clone(), self.eval_const_expr(*value, span, visiting)?));
                }
            }
        }
        if let Some(symbol) = self.named_variant_symbol(record.path.as_ref()) {
            return self
                .eval_named_variant(
                    expr,
                    symbol,
                    fields,
                    &crate::control::GenericBindings::new(),
                    span,
                )
                .map_err(Into::into);
        }
        if let Some(kind) =
            self.primitive_constructor_kind(expr, &crate::control::GenericBindings::new(), span)?
        {
            let (_, input) = fields.pop().expect("checked primitive constructor field");
            return crate::value::representation::construct(kind, input).map_err(|message| {
                ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, message).into()
            });
        }
        let representation = self
            .plan
            .records
            .construct(expr, fields)
            .map_err(|message| {
                ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
            })?;
        if record.path.is_none() || self.plan.records.range_constructor(expr) {
            return Ok(representation);
        }
        let Some(ty) = self.checked.types.expr_types.get(&expr).copied() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "constant nominal record constructor is missing its checked result type",
            )
            .into());
        };
        Ok(InterpValue::Nominal {
            ty,
            value: crate::value::SharedValue::new(representation),
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
    ) -> Result<InterpValue, ConstantFailure> {
        let head = self.eval_const_expr(head, span, visiting)?;
        let tail = self.eval_const_expr(tail, span, visiting)?;
        super::aggregate_expr::prepend_sequence_cell(head, tail, span).map_err(Into::into)
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
            _ => self.checked_sequence_value(expr, Vec::new().into(), None, span),
        }
    }

    fn eval_const_unary_expr(
        &mut self,
        op: HirUnaryOp,
        expr: HirExprId,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ConstantFailure> {
        let value = self.eval_const_expr(expr, span, visiting)?;
        self.eval_unary_value(op, value, span).map_err(Into::into)
    }

    fn eval_const_binary_expr(
        &mut self,
        op: HirBinaryOp,
        lhs: HirExprId,
        rhs: HirExprId,
        span: Span,
        visiting: &mut HashSet<SymbolId>,
    ) -> Result<InterpValue, ConstantFailure> {
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
            .map_err(Into::into)
    }

    pub(super) fn resource_handle_value(
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
