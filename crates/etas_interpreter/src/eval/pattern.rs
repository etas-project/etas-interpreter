use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub(super) fn bind_pattern(
        &self,
        pat: etas_hir::HirPatId,
        value: InterpValue,
        frame: &mut Frame,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        self.bind_borrowed_pattern(pat, &value, frame, span)
    }

    fn bind_borrowed_pattern(
        &self,
        pat: etas_hir::HirPatId,
        value: &InterpValue,
        frame: &mut Frame,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        if self.match_pattern(pat, value, frame, span)? {
            Ok(())
        } else {
            Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "value does not match the checked binding pattern",
            ))
        }
    }

    pub(super) fn match_pattern(
        &self,
        pat: etas_hir::HirPatId,
        value: &InterpValue,
        frame: &mut Frame,
        span: Span,
    ) -> Result<bool, ExecutionFault> {
        let Some(pattern) = self.checked.hir.pats.get(pat) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "pattern is missing from checked HIR",
            ));
        };
        match pattern {
            HirPat::Binding { symbol, .. } => {
                frame.insert(*symbol, value.clone());
                Ok(true)
            }
            HirPat::Wildcard { .. } => Ok(true),
            HirPat::Literal(literal) => self.match_literal_pattern(literal, value, span),
            HirPat::Tuple { elems, .. } => match pattern_projection_value(value) {
                InterpValue::Tuple(values) if values.len() == elems.len() => {
                    for (pat, value) in elems.iter().zip(values.iter()) {
                        if !self.match_pattern(*pat, value, frame, span)? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
                InterpValue::Tuple(_) => Ok(false),
                _ => Ok(false),
            },
            HirPat::Record { path, .. } => {
                if let Some(symbol) = self.named_variant_symbol(path.as_ref()) {
                    let layout = self.plan.named_variants.pattern(pat).ok_or_else(|| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "named enum pattern has no checked slot layout",
                        )
                    })?;
                    if symbol != layout.symbol {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "named enum pattern constructor differs from checked slot layout",
                        ));
                    }
                    let InterpValue::Variant {
                        name,
                        fields: values,
                    } = pattern_projection_value(value)
                    else {
                        return Ok(false);
                    };
                    if name != &layout.name {
                        return Ok(false);
                    }
                    if values.len() != layout.arity {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "enum payload does not match checked pattern arity",
                        ));
                    }
                    for (slot, pat, field_span) in &layout.fields {
                        let value = values.get(*slot).ok_or_else(|| {
                            ExecutionFault::new(
                                AnalysisDiagnosticCode::MissingCheckedFact,
                                *field_span,
                                "invalid checked enum field slot",
                            )
                        })?;
                        if !self.match_pattern(*pat, value, frame, *field_span)? {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
                match pattern_projection_value(value) {
                    InterpValue::Record(values) => {
                        let fields = self.plan.records.pattern(pat).ok_or_else(|| {
                            ExecutionFault::new(
                                AnalysisDiagnosticCode::MissingCheckedFact,
                                span,
                                "record pattern has no checked slot layout",
                            )
                        })?;
                        for field in fields {
                            let field_value = field.field.borrow(values).map_err(|message| {
                                ExecutionFault::new(
                                    AnalysisDiagnosticCode::MissingCheckedFact,
                                    field.span,
                                    message,
                                )
                            })?;
                            if let Some(pat) = field.pat
                                && !self.match_pattern(pat, field_value, frame, field.span)?
                            {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            HirPat::Variant { path, args, .. } => {
                let expected_name = self.variant_name_from_path(path, span)?;
                match pattern_projection_value(value) {
                    InterpValue::OptionNone if expected_name == "None" => Ok(args.is_empty()),
                    InterpValue::OptionSome(inner) if expected_name == "Some" => {
                        Ok(args.len() == 1 && self.match_pattern(args[0], inner, frame, span)?)
                    }
                    InterpValue::Variant { name, fields } if *name == expected_name => {
                        if args.len() != fields.len() {
                            return Ok(false);
                        }
                        for (pat, value) in args.iter().zip(fields.iter()) {
                            if !self.match_pattern(*pat, value, frame, span)? {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            HirPat::Error { .. } => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "error pattern cannot be executed",
            )),
        }
    }

    fn match_literal_pattern(
        &self,
        literal: &HirLiteral,
        value: &InterpValue,
        _span: Span,
    ) -> Result<bool, ExecutionFault> {
        match (literal, value) {
            (
                HirLiteral::Bool {
                    value: expected, ..
                },
                InterpValue::Bool(actual),
            ) => Ok(expected == actual),
            (HirLiteral::Int { text, .. }, InterpValue::Number(actual)) => Ok(
                crate::value::NumericValue::parse_integer(text, actual.primitive())
                    .is_ok_and(|expected| expected == *actual),
            ),
            (
                HirLiteral::String {
                    value: expected, ..
                },
                InterpValue::String(actual),
            ) => Ok(expected == actual),
            (
                HirLiteral::Char {
                    value: expected, ..
                },
                InterpValue::String(actual),
            ) => Ok(actual == &expected.to_string()),
            (HirLiteral::Float { text, .. }, InterpValue::Number(actual)) => Ok(
                crate::value::NumericValue::parse_float(text, actual.primitive())
                    .is_ok_and(|expected| expected == *actual),
            ),
            _ => Ok(false),
        }
    }

    fn variant_name_from_path(
        &self,
        path: &etas_hir::ResolvedPath,
        span: Span,
    ) -> Result<String, ExecutionFault> {
        if let ResolveResult::Resolved(symbol) = path.resolution
            && let Some(constructor) = self.plan.dispatch.enum_constructor(symbol)
        {
            return Ok(constructor.name.clone());
        }
        match path.resolution {
            ResolveResult::Resolved(symbol) => self.checked.symbols.get(symbol).map_or_else(
                || {
                    Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "variant pattern symbol is missing from checked symbol table",
                    ))
                },
                |symbol| Ok(symbol.name.clone()),
            ),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "variant pattern path is not resolved in checked HIR",
            )),
        }
    }

    pub(super) fn bind_handler_patterns(
        &self,
        patterns: &[etas_hir::HirPatId],
        args: &[InterpValue],
        frame: &mut Frame,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        if patterns.len() != args.len() {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "handler arm pattern arity does not match performed action arguments",
            ));
        }
        for (pat, value) in patterns.iter().zip(args) {
            self.bind_borrowed_pattern(*pat, value, frame, span)?;
        }
        Ok(())
    }
}

fn pattern_projection_value(mut value: &InterpValue) -> &InterpValue {
    while let InterpValue::Nominal {
        value: representation,
        ..
    } = value
    {
        value = representation;
    }
    value
}
