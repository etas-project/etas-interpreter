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
        if self.match_pattern(pat, &value, frame, span)? {
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
            HirPat::Record { path, fields, .. } => {
                if let Some(symbol) = self.named_variant_symbol(path.as_ref()) {
                    let Some(symbol) = self.checked.symbols.get(symbol) else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "missing enum pattern symbol",
                        ));
                    };
                    let SymbolDef::EnumVariant {
                        enum_item,
                        variant_index,
                    } = symbol.def
                    else {
                        unreachable!()
                    };
                    let Some(HirItem::Enum(decl)) = self.checked.hir.items.get(enum_item) else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "missing enum pattern declaration",
                        ));
                    };
                    let Some(names) = decl
                        .variants
                        .get(variant_index as usize)
                        .and_then(|v| v.field_names.as_ref())
                    else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "missing named enum pattern layout",
                        ));
                    };
                    let InterpValue::Variant {
                        name,
                        fields: values,
                    } = pattern_projection_value(value)
                    else {
                        return Ok(false);
                    };
                    if name != &symbol.name {
                        return Ok(false);
                    }
                    for field in fields {
                        let Some(value) = names
                            .iter()
                            .position(|name| name == &field.name)
                            .and_then(|index| values.get(index))
                        else {
                            return Err(ExecutionFault::new(
                                AnalysisDiagnosticCode::MissingCheckedFact,
                                span,
                                "missing enum payload field",
                            ));
                        };
                        if let Some(pat) = field.pat {
                            if !self.match_pattern(pat, value, frame, field.span)? {
                                return Ok(false);
                            }
                        }
                    }
                    return Ok(true);
                }
                match pattern_projection_value(value) {
                    InterpValue::Record(values) => {
                        let values = values.borrow();
                        for field in fields {
                            let Some((_, field_value)) =
                                values.iter().find(|(name, _)| name == &field.name)
                            else {
                                return Ok(false);
                            };
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
        for (pat, value) in patterns.iter().zip(args.iter().cloned()) {
            self.bind_pattern(*pat, value, frame, span)?;
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
