use super::*;
use etas_frontend::CheckedCallableRoot;
use etas_types::{CallableSignature, EffectRowRef, ItemSignature, SymbolTypeFact};

impl SnapshotValidator<'_> {
    pub(super) fn callable_leaf_interface(
        &self,
        target: &Target,
        bindings: &Bindings,
        allowed: &Option<EffectRowRef>,
    ) -> Result<Interface, String> {
        let (root, signature, component, bindings) = match target {
            Target::FlowItem(item) | Target::ToolItem(item) => {
                let signature = match (target, self.checked.types.item_signatures.get(item)) {
                    (Target::FlowItem(_), Some(ItemSignature::Flow(signature)))
                    | (Target::ToolItem(_), Some(ItemSignature::Tool(signature))) => signature,
                    _ => {
                        return Err(
                            "checkpoint callable kind disagrees with its checked item".into()
                        );
                    }
                };
                (
                    CheckedCallableRoot::Item(*item),
                    flow_type(signature),
                    None,
                    bindings.clone(),
                )
            }
            Target::AgentItem(item) => return self.agent_interface(*item, bindings, allowed),
            Target::AgentRevision(reference) => {
                let agent = self
                    .revisions
                    .ok_or("checkpoint agent reference requires freshly prepared revisions")?
                    .restore(reference, self.checked)?;
                if !agent.is_default_entry() {
                    return Err(
                        "checkpoint auxiliary agent method is not a nominal agent value".into(),
                    );
                }
                return self.agent_interface(agent.source_agent()?, bindings, allowed);
            }
            Target::SpecImplMethod(symbol) => {
                let Some(etas_hir::SymbolDef::Item { item }) =
                    self.checked.symbols.get(*symbol).map(|symbol| &symbol.def)
                else {
                    return Err("checkpoint spec method has no checked item".into());
                };
                let Some(ItemSignature::Flow(signature)) =
                    self.checked.types.item_signatures.get(item)
                else {
                    return Err("checkpoint spec method has no checked signature".into());
                };
                (
                    CheckedCallableRoot::Item(*item),
                    flow_type(signature),
                    None,
                    bindings.clone(),
                )
            }
            Target::Lambda { expr, captured } => {
                let mut bindings = bindings.clone();
                for (name, ty) in &captured.type_bindings {
                    bindings.entry(name.clone()).or_insert(*ty);
                }
                for (name, row) in &captured.effect_row_bindings {
                    bindings
                        .rows
                        .entry(name.clone())
                        .or_insert_with(|| row.clone());
                }
                (
                    CheckedCallableRoot::Lambda(*expr),
                    self.expression_signature(*expr)?,
                    None,
                    bindings,
                )
            }
            Target::PureIntrinsic {
                intrinsic,
                parameter_types,
                result_type,
            } => {
                let root = self.dispatch.intrinsic_callable_root(
                    crate::intrinsic::dispatch::StdIntrinsicIdentity {
                        intrinsic: *intrinsic,
                        dispatch: etas_std::IntrinsicDispatch::PureKernel,
                    },
                    parameter_types,
                    *result_type,
                )?;
                (
                    root,
                    FlowType {
                        input: parameter_types.clone(),
                        output: *result_type,
                        effects: None,
                    },
                    None,
                    bindings.clone(),
                )
            }
            Target::StdIntrinsic {
                origin,
                substitutions,
                intrinsic,
                dispatch,
                parameter_types,
                result_type,
            } => {
                let call = crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
                    origin: *origin,
                    substitutions: substitutions.clone(),
                    identity: crate::intrinsic::dispatch::StdIntrinsicIdentity {
                        intrinsic: *intrinsic,
                        dispatch: *dispatch,
                    },
                    parameter_types: parameter_types.clone(),
                    result_type: *result_type,
                };
                let root = self.dispatch.std_callable_root(&call)?;
                let mut bindings = bindings.clone();
                for step in substitutions {
                    bindings.extend(step.iter().cloned());
                }
                (
                    root,
                    FlowType {
                        input: parameter_types.clone(),
                        output: *result_type,
                        effects: None,
                    },
                    None,
                    bindings,
                )
            }
            Target::Scoped { metadata, target } => {
                let (_, projections) = self.scoped_callable(metadata, target)?;
                let presented = projections
                    .last()
                    .map_or(metadata.native_type, |edge| edge.target().1);
                let Some(Type::Function(signature)) = self.checked.type_store.get(presented) else {
                    return Err(
                        "checkpoint scoped callable has no checked presented interface".into(),
                    );
                };
                self.validate_presented_effects(signature, allowed)?;
                return Ok(Interface {
                    signature: Some(signature.clone().into()),
                    component: None,
                });
            }
            Target::EnumVariant(symbol) => {
                let Some(SymbolTypeFact::Flow { signature }) =
                    self.checked.types.symbol_types.get(symbol)
                else {
                    return Err("checkpoint constructor has no checked signature".into());
                };
                let span = self
                    .checked
                    .symbols
                    .get(*symbol)
                    .ok_or("checkpoint constructor symbol is missing")?
                    .definition_span;
                crate::value::validate_snapshot_bounds(
                    self.checked,
                    signature,
                    span,
                    bindings,
                    self.snapshot_callable_specs(),
                )?;
                return Ok(Interface {
                    signature: Some(substitute(self, flow_type(signature), bindings)?.into()),
                    component: None,
                });
            }
            Target::NominalConstructor(ty) => {
                let representation = crate::plan::code::CheckedValueTypes::default().representation(self.checked, *ty)
                    .map_err(|error| format!("checkpoint nominal constructor has no checked representation: {error:?}"))?;
                return Ok(Interface {
                    signature: Some(CallableAbi {
                        input: vec![representation],
                        output: *ty,
                    }),
                    component: Some(*ty),
                });
            }
            Target::Composed(_) | Target::Specialized { .. } | Target::Limited { .. } => {
                return Err("checkpoint callable graph contains an unprocessed wrapper".into());
            }
        };
        let signature = substitute(self, signature, &bindings)?;
        let expected = FlowType {
            effects: allowed.clone(),
            ..signature.clone()
        };
        crate::value::validate_snapshot_signature(
            self.checked,
            root,
            &expected,
            &bindings,
            self.snapshot_callable_specs(),
        )?;
        Ok(Interface {
            signature: Some(signature.into()),
            component,
        })
    }

    fn expression_signature(&self, expr: etas_hir::HirExprId) -> Result<FlowType, String> {
        let Some(Type::Function(signature)) = self
            .checked
            .types
            .expr_types
            .get(&expr)
            .and_then(|ty| self.checked.type_store.get(*ty))
        else {
            return Err("checkpoint callable expression has no checked signature".into());
        };
        Ok(signature.clone())
    }

    fn agent_interface(
        &self,
        item: etas_hir::HirItemId,
        bindings: &Bindings,
        allowed: &Option<EffectRowRef>,
    ) -> Result<Interface, String> {
        let Some(ItemSignature::Agent(agent)) = self.checked.types.item_signatures.get(&item)
        else {
            return Err("checkpoint agent has no checked nominal identity".into());
        };
        let signature = agent
            .callable()
            .map(|signature| substitute(self, flow_type(signature), bindings))
            .transpose()?;
        if let Some(signature) = &signature {
            let expected = FlowType {
                effects: allowed.clone(),
                ..signature.clone()
            };
            crate::value::validate_snapshot_signature(
                self.checked,
                CheckedCallableRoot::Item(item),
                &expected,
                bindings,
                self.snapshot_callable_specs(),
            )?;
        }
        Ok(Interface {
            signature: signature.map(Into::into),
            component: Some(agent.component),
        })
    }

    fn validate_presented_effects(
        &self,
        signature: &FlowType,
        allowed: &Option<EffectRowRef>,
    ) -> Result<(), String> {
        if let Some(allowed) = allowed {
            let actual = signature
                .effects
                .as_ref()
                .ok_or("checkpoint scoped callable has no bounded checked effect row")?;
            let row = |row| {
                etas_effects::validate::resolve_checked_type_row(
                    &self.checked.type_store,
                    &self.checked.effect_registry,
                    row,
                )
            };
            if !(etas_effects::EffectCoverage {
                registry: &self.checked.effect_registry,
                types: &self.checked.type_store,
            })
            .row_covers(&row(allowed)?, &row(actual)?)
            {
                return Err("checkpoint callable effects exceed its checked slot interface".into());
            }
        }
        Ok(())
    }
}

fn flow_type(signature: &CallableSignature) -> FlowType {
    FlowType {
        input: signature.params.clone(),
        output: signature.output,
        effects: signature.effects.clone(),
    }
}

pub(super) fn substitute(
    validator: &SnapshotValidator<'_>,
    signature: FlowType,
    bindings: &Bindings,
) -> Result<FlowType, String> {
    let mut substitution = bindings.substitution(&validator.checked.type_store);
    Ok(FlowType {
        input: signature
            .input
            .into_iter()
            .map(|ty| {
                substitution
                    .substitute(ty)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<_, _>>()?,
        output: substitution
            .substitute(signature.output)
            .map_err(|error| error.to_string())?,
        effects: signature
            .effects
            .map(|row| {
                substitution
                    .substitute_effect_row(row)
                    .map_err(|error| error.to_string())
            })
            .transpose()?,
    })
}
