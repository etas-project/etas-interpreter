use super::*;
use etas_frontend::AstItemRef;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_std_callable_value(
        &mut self,
        expr: HirExprId,
        frame: &Frame,
        span: Span,
    ) -> Result<Option<InterpValue>, crate::control::ExecutionFault> {
        let Some(HirExpr::Path(path)) = self.checked.hir.exprs.get(expr) else {
            return Ok(None);
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Ok(None);
        };
        if frame.get(symbol).is_some()
            || self.std_intrinsic(symbol).is_none()
            || !matches!(
                self.checked
                    .types
                    .expr_types
                    .get(&expr)
                    .and_then(|ty| self.checked.type_store.get(*ty)),
                Some(etas_types::Type::Function(_))
            )
        {
            return Ok(None);
        }
        let missing = |message| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                message,
            )
        };
        self.checked
            .checked_callable_owner(etas_frontend::CheckedCallableRoot::StdReference(expr))
            .map_err(|error| missing(error.to_string()))?;
        let target = self
            .resolve_static_call_target(expr, frame, span)?
            .ok_or_else(|| {
                missing("checked standard callable reference has no native target".to_owned())
            })?;
        Ok(Some(InterpValue::Callable(target)))
    }

    pub(super) fn source_agent_call_target(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Result<CallTarget, crate::control::ExecutionFault> {
        if matches!(self.checked.types.item_signatures.get(&item), Some(etas_types::ItemSignature::Agent(agent)) if agent.default_entry.is_none())
        {
            return Ok(CallTarget::AgentItem(item));
        }
        match self.revision {
            None => Ok(CallTarget::AgentItem(item)),
            Some(revision) => crate::value::AgentRevisionValue::frozen_declaration(
                revision.clone(),
                self.checked,
                item,
            )
            .map(CallTarget::AgentRevision)
            .map_err(|reason| {
                crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    reason,
                )
            }),
        }
    }
    pub(super) fn source_flow_call_target(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Result<CallTarget, crate::control::ExecutionFault> {
        if let Some(revision) = self.revision
            && revision.targets.frozen_methods.contains_key(&item)
        {
            return crate::value::AgentRevisionValue::frozen_method(
                revision.clone(),
                self.checked,
                item,
            )
            .map(CallTarget::AgentRevision)
            .map_err(|reason| {
                crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    reason,
                )
            });
        }
        Ok(CallTarget::FlowItem(item))
    }

    pub(super) fn source_tool_call_target(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Result<CallTarget, crate::control::ExecutionFault> {
        if matches!(self.checked.hir.items.get(item), Some(HirItem::Tool(tool)) if matches!(tool.body, HirToolBody::Source(_)))
            && matches!(
                self.checked.types.item_signatures.get(&item),
                Some(etas_types::ItemSignature::Tool(_))
            )
        {
            return Ok(CallTarget::ToolItem(item));
        }
        Err(crate::control::ExecutionFault::new(
            AnalysisDiagnosticCode::MissingCheckedFact,
            span,
            "tool value has no checked source callable body",
        ))
    }
    pub(super) fn specialize_call_target_for_call(
        &self,
        call: HirExprId,
        target: CallTarget,
        frame: &Frame,
        span: Span,
    ) -> Result<CallTarget, crate::control::ExecutionFault> {
        let target = match target {
            CallTarget::PureIntrinsic(call) => CallTarget::PureIntrinsic(
                call.specialize(&self.checked.type_store, frame.generic_bindings())
                    .map_err(|message| {
                        crate::control::ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            message,
                        )
                    })?,
            ),
            CallTarget::StdIntrinsic(call) => CallTarget::StdIntrinsic(
                call.specialize(&self.checked.type_store, frame.type_bindings())
                    .map_err(|message| {
                        crate::control::ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            message,
                        )
                    })?,
            ),
            other => other,
        };
        let bindings = crate::control::CheckedCallBindings::resolve(
            self.checked,
            call,
            frame.generic_bindings(),
        )
        .map_err(|error| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!("checked call bindings cannot be materialized: {error}"),
            )
        })?;
        if bindings.types.is_empty() && bindings.rows.is_empty() {
            return Ok(target);
        }
        Ok(CallTarget::Specialized {
            target: target.into(),
            type_bindings: bindings.types,
            effect_row_bindings: bindings.rows,
        })
    }

    pub(super) fn resolve_nominal_constructor_call_target(
        &self,
        call: HirExprId,
        callee: HirExprId,
        frame: &Frame,
        span: Span,
    ) -> Result<Option<CallTarget>, crate::control::ExecutionFault> {
        let HirExpr::Path(path) = &self.checked.hir.exprs[callee] else {
            return Ok(None);
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Ok(None);
        };
        if frame.get(symbol).is_some() {
            return Ok(None);
        }
        let (constructor, alias) = match self.checked.types.symbol_types.get(&symbol) {
            Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) => {
                (*constructor, false)
            }
            Some(etas_types::SymbolTypeFact::TypeAlias { target, .. }) => {
                let base = match self.checked.type_store.get(*target) {
                    Some(etas_types::Type::Applied { constructor, .. }) => *constructor,
                    Some(etas_types::Type::Nominal(_)) => etas_types::TypeConstructorId(target.0),
                    _ => return Ok(None),
                };
                (base, true)
            }
            _ => return Ok(None),
        };
        let Some(result_type) = self.checked.types.expr_types.get(&call).copied() else {
            return Err(crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "nominal constructor call is missing its checked result type",
            ));
        };
        let matches_constructor = match self.checked.type_store.get(result_type) {
            Some(etas_types::Type::Nominal(_)) => result_type.0 == constructor.0,
            Some(etas_types::Type::Applied {
                constructor: applied,
                ..
            }) => *applied == constructor,
            _ => false,
        };
        if !matches_constructor
            || (alias && self.checked.types.expr_types.get(&callee).copied() != Some(result_type))
        {
            return Err(crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "nominal constructor result type does not match its checked constructor symbol",
            ));
        }
        Ok(Some(CallTarget::NominalConstructor(result_type)))
    }

    pub(super) fn resolve_static_call_target_for_call(
        &self,
        call: HirExprId,
        callee: HirExprId,
        args: &[HirArg],
        frame: &Frame,
        span: Span,
    ) -> Result<Option<CallTarget>, crate::control::ExecutionFault> {
        if let Some(checked) = self.plan.dispatch.std_type_member(call) {
            return Ok(Some(CallTarget::StdIntrinsic(checked.clone())));
        }
        let HirExpr::Path(path) = &self.checked.hir.exprs[callee] else {
            return Ok(None);
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Ok(None);
        };
        if frame.get(symbol).is_some() {
            return Ok(None);
        }
        let Some(symbol_data) = self.checked.symbols.get(symbol) else {
            return Ok(None);
        };
        Ok(match &symbol_data.def {
            SymbolDef::Item { item } if symbol_data.kind == SymbolKind::Flow => Some(
                if matches!(self.checked.hir.items.get(*item), Some(HirItem::Agent(_))) {
                    self.source_agent_call_target(*item, span)?
                } else {
                    self.source_flow_call_target(*item, span)?
                },
            ),
            SymbolDef::Item { item } if symbol_data.kind == SymbolKind::Agent => {
                Some(self.source_agent_call_target(*item, span)?)
            }
            SymbolDef::Item { item } if symbol_data.kind == SymbolKind::Tool => {
                Some(self.source_tool_call_target(*item, span)?)
            }
            SymbolDef::ImportAlias { path, .. } => {
                if self.plan.dispatch.enum_constructor(symbol).is_some() {
                    return Ok(Some(CallTarget::EnumVariant(symbol)));
                }
                if let Some(intrinsic) = self.std_intrinsic(symbol) {
                    return match intrinsic.dispatch {
                        etas_std::IntrinsicDispatch::PureKernel => self
                            .checked_pure_intrinsic_call(call, args, intrinsic.intrinsic, span)
                            .map(|call| Some(CallTarget::PureIntrinsic(call))),
                        _ => self
                            .checked_std_intrinsic_call(call, args, intrinsic, span)
                            .map(|call| Some(CallTarget::StdIntrinsic(call))),
                    };
                }
                match self.source_item_id_for_import_path(path) {
                    Some(item) => match self.checked.hir.items.get(item) {
                        Some(HirItem::Flow(_)) => Some(self.source_flow_call_target(item, span)?),
                        Some(HirItem::Agent(_)) => Some(self.source_agent_call_target(item, span)?),
                        Some(HirItem::Tool(_)) => Some(self.source_tool_call_target(item, span)?),
                        _ => None,
                    },
                    None => None,
                }
            }
            _ if symbol_data.kind == SymbolKind::EnumVariant => {
                Some(CallTarget::EnumVariant(symbol))
            }
            _ => None,
        })
    }

    pub(super) fn resolve_static_call_target(
        &self,
        callee: HirExprId,
        frame: &Frame,
        span: Span,
    ) -> Result<Option<CallTarget>, crate::control::ExecutionFault> {
        let HirExpr::Path(path) = &self.checked.hir.exprs[callee] else {
            return Ok(None);
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Ok(None);
        };
        if frame.get(symbol).is_some() {
            return Ok(None);
        }
        let Some(symbol_data) = self.checked.symbols.get(symbol) else {
            return Ok(None);
        };
        Ok(match &symbol_data.def {
            SymbolDef::Item { item } if symbol_data.kind == SymbolKind::Flow => Some(
                if matches!(self.checked.hir.items.get(*item), Some(HirItem::Agent(_))) {
                    self.source_agent_call_target(*item, span)?
                } else {
                    self.source_flow_call_target(*item, span)?
                },
            ),
            SymbolDef::Item { item } if symbol_data.kind == SymbolKind::Agent => {
                Some(self.source_agent_call_target(*item, span)?)
            }
            SymbolDef::Item { item } if symbol_data.kind == SymbolKind::Tool => {
                Some(self.source_tool_call_target(*item, span)?)
            }
            SymbolDef::ImportAlias { path, .. } => {
                if self.plan.dispatch.enum_constructor(symbol).is_some() {
                    return Ok(Some(CallTarget::EnumVariant(symbol)));
                }
                if let Some(intrinsic) = self.std_intrinsic(symbol) {
                    return match intrinsic.dispatch {
                        etas_std::IntrinsicDispatch::PureKernel => self
                            .checked_pure_intrinsic_callable(callee, intrinsic.intrinsic, span)
                            .map(|call| Some(CallTarget::PureIntrinsic(call))),
                        _ => self
                            .checked_std_intrinsic_callable(callee, intrinsic, span)
                            .map(|call| Some(CallTarget::StdIntrinsic(call))),
                    };
                }
                match self.source_item_id_for_import_path(path) {
                    Some(item) => match self.checked.hir.items.get(item) {
                        Some(HirItem::Flow(_)) => Some(self.source_flow_call_target(item, span)?),
                        Some(HirItem::Agent(_)) => Some(self.source_agent_call_target(item, span)?),
                        Some(HirItem::Tool(_)) => Some(self.source_tool_call_target(item, span)?),
                        _ => None,
                    },
                    None => None,
                }
            }
            _ if symbol_data.kind == SymbolKind::EnumVariant => {
                Some(CallTarget::EnumVariant(symbol))
            }
            _ => None,
        })
    }

    fn checked_pure_intrinsic_call(
        &self,
        call: HirExprId,
        args: &[HirArg],
        intrinsic: etas_std::StdIntrinsicId,
        span: Span,
    ) -> Result<crate::intrinsic::dispatch::CheckedPureIntrinsicCall, crate::control::ExecutionFault>
    {
        let parameter_types = args
            .iter()
            .map(|arg| match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => self
                    .checked
                    .types
                    .expr_types
                    .get(expr)
                    .copied()
                    .ok_or_else(|| {
                        crate::control::ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            format!(
                                "pure intrinsic argument expression {} is missing its checked type",
                                expr.0
                            ),
                        )
                    }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let result_type = self
            .checked
            .types
            .expr_types
            .get(&call)
            .copied()
            .ok_or_else(|| {
                crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "pure intrinsic call expression {} is missing its checked result type",
                        call.0
                    ),
                )
            })?;
        Ok(crate::intrinsic::dispatch::CheckedPureIntrinsicCall {
            intrinsic,
            parameter_types,
            result_type,
        })
    }

    fn checked_pure_intrinsic_callable(
        &self,
        callee: HirExprId,
        intrinsic: etas_std::StdIntrinsicId,
        span: Span,
    ) -> Result<crate::intrinsic::dispatch::CheckedPureIntrinsicCall, crate::control::ExecutionFault>
    {
        let callable_type = self
            .checked
            .types
            .expr_types
            .get(&callee)
            .copied()
            .ok_or_else(|| {
                crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "pure intrinsic callable expression {} is missing its checked type",
                        callee.0
                    ),
                )
            })?;
        let Some(etas_types::Type::Function(flow)) = self.checked.type_store.get(callable_type)
        else {
            return Err(crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!(
                    "pure intrinsic callable expression {} does not have a checked flow type",
                    callee.0
                ),
            ));
        };
        Ok(crate::intrinsic::dispatch::CheckedPureIntrinsicCall {
            intrinsic,
            parameter_types: flow.input.clone(),
            result_type: flow.output,
        })
    }

    fn checked_std_intrinsic_call(
        &self,
        call: HirExprId,
        args: &[HirArg],
        identity: crate::intrinsic::dispatch::StdIntrinsicIdentity,
        span: Span,
    ) -> Result<crate::intrinsic::dispatch::CheckedStdIntrinsicCall, crate::control::ExecutionFault>
    {
        let parameter_types = args
            .iter()
            .map(|arg| match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => self
                    .checked
                    .types
                    .expr_types
                    .get(expr)
                    .copied()
                    .ok_or_else(|| {
                        crate::control::ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            format!(
                                "standard intrinsic argument expression {} is missing its checked type",
                                expr.0
                            ),
                        )
                    }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let result_type = self
            .checked
            .types
            .expr_types
            .get(&call)
            .copied()
            .ok_or_else(|| {
                crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "standard intrinsic call expression {} is missing its checked result type",
                        call.0
                    ),
                )
            })?;
        Ok(crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
            identity,
            origin: call,
            substitutions: Vec::new(),
            parameter_types,
            result_type,
        })
    }

    fn checked_std_intrinsic_callable(
        &self,
        callee: HirExprId,
        identity: crate::intrinsic::dispatch::StdIntrinsicIdentity,
        span: Span,
    ) -> Result<crate::intrinsic::dispatch::CheckedStdIntrinsicCall, crate::control::ExecutionFault>
    {
        let callable_type = self
            .checked
            .types
            .expr_types
            .get(&callee)
            .copied()
            .ok_or_else(|| {
                crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "standard intrinsic callable expression {} is missing its checked type",
                        callee.0
                    ),
                )
            })?;
        let Some(etas_types::Type::Function(flow)) = self.checked.type_store.get(callable_type)
        else {
            return Err(crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!(
                    "standard intrinsic callable expression {} does not have a checked flow type",
                    callee.0
                ),
            ));
        };
        Ok(crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
            identity,
            origin: callee,
            substitutions: Vec::new(),
            parameter_types: flow.input.clone(),
            result_type: flow.output,
        })
    }

    pub(super) fn source_item_id_for_import_path(&self, path: &[String]) -> Option<HirItemId> {
        self.source_item_for_import_path(path)
            .and_then(|ast_item| self.hir_item_for_ast_item(&ast_item))
            .or_else(|| self.canonical_source_item_for_import_path(path))
    }

    pub(super) fn source_item_for_import_path(&self, path: &[String]) -> Option<AstItemRef> {
        let (member, module_segments) = path.split_last()?;
        let module_path = etas_frontend::ModulePath {
            segments: module_segments.to_vec(),
        };
        let module_id = self.checked.module_index.by_path.get(&module_path)?;
        let module = self.checked.module_index.modules.get(*module_id)?;
        let export = module.visibility_exports.items.get(member)?;
        Some(export.item.clone())
    }

    pub(super) fn hir_item_for_ast_item(&self, ast_item: &AstItemRef) -> Option<HirItemId> {
        self.checked.hir.items.iter().find_map(|(id, item)| {
            let span = item.span();
            (span == ast_item.span && span.source == ast_item.source).then_some(id)
        })
    }

    fn canonical_source_item_for_import_path(&self, path: &[String]) -> Option<HirItemId> {
        self.checked.hir.symbols.iter().find_map(|symbol| {
            if !matches!(
                symbol.kind,
                SymbolKind::Flow
                    | SymbolKind::Agent
                    | SymbolKind::Tool
                    | SymbolKind::TopLevelLet
                    | SymbolKind::Spec
                    | SymbolKind::Protocol
                    | SymbolKind::Effect
                    | SymbolKind::EffectAction
            ) {
                return None;
            }
            let SymbolDef::Item { item } = symbol.def else {
                return None;
            };
            let module = self.checked.hir.modules_arena.get(symbol.defining_module)?;
            let module_name = module.name.as_ref()?;
            let mut canonical_path = module_name
                .segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect::<Vec<_>>();
            canonical_path.push(
                symbol
                    .name
                    .rsplit('.')
                    .next()
                    .unwrap_or(&symbol.name)
                    .to_owned(),
            );
            (canonical_path == path).then_some(item)
        })
    }
}
