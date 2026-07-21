use super::*;
use etas_frontend::AstItemRef;

impl<'a> EvalContext<'a> {
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
        let Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) =
            self.checked.types.symbol_types.get(&symbol)
        else {
            return Ok(None);
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
            }) => applied == constructor,
            _ => false,
        };
        if !matches_constructor {
            return Err(crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "nominal constructor result type does not match its checked constructor symbol",
            ));
        }
        Ok(Some(CallTarget::NominalConstructor(result_type)))
    }

    pub(super) fn resolve_static_call_target(
        &self,
        callee: HirExprId,
        frame: &Frame,
    ) -> Option<CallTarget> {
        let HirExpr::Path(path) = &self.checked.hir.exprs[callee] else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        if frame.get(symbol).is_some() {
            return None;
        }
        let symbol_data = self.checked.symbols.get(symbol)?;
        match &symbol_data.def {
            SymbolDef::Item { item } if symbol_data.kind == SymbolKind::Flow => {
                Some(CallTarget::FlowItem(*item))
            }
            SymbolDef::Item { item } if symbol_data.kind == SymbolKind::Agent => {
                Some(CallTarget::AgentItem(*item))
            }
            SymbolDef::ImportAlias { path, .. } => {
                if let Some(kind) = self.std_callable_for_path(path) {
                    return Some(CallTarget::StdCallable(kind));
                }
                let item = self.source_item_id_for_import_path(path)?;
                match self.checked.hir.items.get(item) {
                    Some(HirItem::Flow(_)) => Some(CallTarget::FlowItem(item)),
                    Some(HirItem::Agent(_)) => Some(CallTarget::AgentItem(item)),
                    _ => None,
                }
            }
            _ if symbol_data.kind == SymbolKind::EnumVariant => {
                Some(CallTarget::EnumVariant(symbol))
            }
            _ => None,
        }
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
