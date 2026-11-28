use std::{collections::BTreeMap, sync::Arc};

use etas_effects::ActionRef;
use etas_frontend::code::definition::{DefinitionCatalog, NativeActionSignature};
use etas_types::TypeId;

use super::{RevisionTypeBridgeError as Error, runtime::RuntimeTypeBridge};

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct ActionBridge {
    forward: BTreeMap<ActionRef, Arc<ActionPair>>,
    backward: BTreeMap<ActionRef, Arc<ActionPair>>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct ActionPair {
    pub original: ActionRef,
    pub candidate: ActionRef,
}

impl ActionBridge {
    pub fn new(
        original: &DefinitionCatalog,
        candidate: &DefinitionCatalog,
        types: &RuntimeTypeBridge,
    ) -> Result<Self, Error> {
        let source = original
            .native_action_signatures()
            .map_err(Error::Projection)?;
        let target = candidate
            .native_action_signatures()
            .map_err(Error::Projection)?;
        let mut targets = BTreeMap::<_, Vec<_>>::new();
        for action in &target {
            targets
                .entry((&action.effect, action.name.as_str()))
                .or_default()
                .push(action);
        }
        let mut forward = BTreeMap::<ActionRef, Arc<ActionPair>>::new();
        let mut backward = BTreeMap::new();
        for from in source {
            let Some(matches) = targets.get(&(&from.effect, from.name.as_str())) else {
                continue;
            };
            let mut matched = None;
            let mut mismatch =
                invalid("checked action overload is absent from the frozen environment");
            for to in matches {
                if let (Some(from), Some(to)) = (from.overload, to.overload)
                    && from != to
                {
                    continue;
                }
                match validate(original, candidate, &from, to, types) {
                    Ok(()) => {
                        matched = Some(*to);
                        break;
                    }
                    Err(error) => mismatch = error,
                }
            }
            let to = matched.ok_or(mismatch)?;
            if let Some(existing) = forward.get(&from.action) {
                if existing.candidate != to.action {
                    return Err(invalid("ambiguous native action correspondence"));
                }
                continue;
            }
            let pair = Arc::new(ActionPair {
                original: from.action.clone(),
                candidate: to.action.clone(),
            });
            forward.insert(from.action, pair.clone());
            if backward.insert(to.action.clone(), pair).is_some() {
                return Err(invalid("ambiguous native action correspondence"));
            }
        }
        let original_program = original.checked_program();
        let candidate_program = candidate.checked_program();
        // A callback can carry its caller's action through a generic std body.
        // Its payload ABI remains in the invocation's checked origin; this
        // correspondence transports only the canonical row member identity.
        let carried_targets = candidate_program
            .effect_registry
            .action_identities()
            .map(|(action, name)| {
                Ok((
                    (
                        candidate
                            .native_effect_identity(action.tag)
                            .map_err(Error::Projection)?
                            .clone(),
                        name.to_owned(),
                    ),
                    action,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        for (from, name) in original_program.effect_registry.action_identities() {
            let owner = original
                .native_effect_identity(from.tag)
                .map_err(Error::Projection)?;
            let Some(to) = carried_targets.get(&(owner.clone(), name.to_owned())) else {
                continue;
            };
            if forward.contains_key(&from)
                || !(original_program.effect_registry.is_carried_action(&from)
                    || candidate_program.effect_registry.is_carried_action(to))
            {
                continue;
            }
            if original_program.effect_registry.action_selectors(&from)
                != candidate_program.effect_registry.action_selectors(to)
            {
                return Err(invalid(
                    "carried action selector shape changed across the checked environment",
                ));
            }
            let pair = Arc::new(ActionPair {
                original: from.clone(),
                candidate: to.clone(),
            });
            forward.insert(from, pair.clone());
            if backward.insert(to.clone(), pair).is_some() {
                return Err(invalid("ambiguous carried action correspondence"));
            }
        }
        for symbol in original_program.std_registry.symbols() {
            let etas_std::StdDecl::EffectAction(declaration) = &symbol.decl else {
                continue;
            };
            let path = symbol.qualified_path.join(".");
            let (Some(from), Some(to)) = (
                standard_reference(
                    &original_program.effect_registry,
                    &path,
                    declaration.stable_id,
                ),
                standard_reference(
                    &candidate_program.effect_registry,
                    &path,
                    declaration.stable_id,
                ),
            ) else {
                continue;
            };
            if declaration.stable_id.is_some() {
                let Some(other) = candidate_program
                    .std_registry
                    .lookup_qualified(&symbol.qualified_path)
                else {
                    continue;
                };
                let etas_std::StdDecl::EffectAction(other) = &other.decl else {
                    return Err(invalid("standard action execution contract is missing"));
                };
                if execution_contract(declaration) != execution_contract(other) {
                    return Err(invalid(
                        "standard action execution contract changed across the frozen environment",
                    ));
                }
            } else {
                // A source-produced library registers its own actions in HIR.
                // The consumer receives their checked metadata through std.
                let (left, right) = original_program
                    .effect_registry
                    .action_signature(&from)
                    .zip(candidate_program.effect_registry.action_signature(&to))
                    .ok_or_else(|| invalid("source standard action has no checked signature"))?;
                if left.effect_args != right.effect_args
                    || left.selector_param_names != right.selector_param_names
                    || left.has_default_handler != right.has_default_handler
                    || left.returns_never != right.returns_never
                {
                    return Err(invalid(
                        "source standard action contract changed across the checked environment",
                    ));
                }
            }
            if original
                .native_effect_identity(from.tag)
                .map_err(Error::Projection)?
                != candidate
                    .native_effect_identity(to.tag)
                    .map_err(Error::Projection)?
                || original_program
                    .effect_registry
                    .action_name(from.tag, from.action)
                    != candidate_program
                        .effect_registry
                        .action_name(to.tag, to.action)
            {
                return Err(invalid(
                    "standard action identity changed across the frozen environment",
                ));
            }
            if let Some(existing) = forward.get(&from) {
                if existing.candidate != to {
                    return Err(invalid("ambiguous standard action correspondence"));
                }
                continue;
            }
            let pair = Arc::new(ActionPair {
                original: from.clone(),
                candidate: to.clone(),
            });
            forward.insert(from, pair.clone());
            if backward.insert(to, pair).is_some() {
                return Err(invalid("ambiguous standard action correspondence"));
            }
        }
        Ok(Self { forward, backward })
    }

    pub fn to_candidate(&self, action: &ActionRef) -> Result<&Arc<ActionPair>, Error> {
        self.forward.get(action).ok_or_else(|| {
            invalid(&format!(
                "action {action:?} has no checked candidate declaration"
            ))
        })
    }

    pub fn to_original(&self, action: &ActionRef) -> Result<&Arc<ActionPair>, Error> {
        self.backward.get(action).ok_or_else(|| {
            invalid(&format!(
                "action {action:?} has no checked original declaration"
            ))
        })
    }

    pub fn corresponding_action(&self, action: &ActionRef, forward: bool) -> Option<ActionRef> {
        if forward {
            self.forward.get(action).map(|pair| pair.candidate.clone())
        } else {
            self.backward.get(action).map(|pair| pair.original.clone())
        }
    }
}

fn standard_reference(
    registry: &etas_effects::EffectRegistry,
    path: &str,
    stable_id: Option<u32>,
) -> Option<ActionRef> {
    match stable_id {
        Some(id) => registry
            .standard_action(etas_effects::EffectActionId(id))
            .map(|signature| ActionRef {
                tag: signature.owner,
                action: signature.id,
            }),
        None => registry.action_by_name(path),
    }
}

fn validate(
    original: &DefinitionCatalog,
    candidate: &DefinitionCatalog,
    from: &NativeActionSignature<'_>,
    to: &NativeActionSignature<'_>,
    types: &RuntimeTypeBridge,
) -> Result<(), Error> {
    let (a, b) = (from.signature, to.signature);
    if a.generic_params.len() != b.generic_params.len()
        || a.params.len() != b.params.len()
        || a.effect_args != b.effect_args
        || a.selector_param_names != b.selector_param_names
        || a.returns_never != b.returns_never
        || a.selector_defaults != b.selector_defaults
    {
        return Err(invalid(
            "action declaration ABI changed across the frozen environment",
        ));
    }
    let parameters = a
        .generic_params
        .iter()
        .zip(&b.generic_params)
        .map(|(a, b)| {
            if a.name != b.name || a.kind != b.kind || a.bounds.len() != b.bounds.len() {
                return Err(invalid("action generic parameter ABI changed"));
            }
            Ok((a.subject, b.subject))
        })
        .collect::<Result<Vec<(TypeId, TypeId)>, Error>>()?;
    let effect_parameters = a
        .generic_params
        .iter()
        .zip(&b.generic_params)
        .filter(|(parameter, _)| parameter.kind == etas_types::CallableGenericParamKind::Effect)
        .map(|(from, to)| (from.name.clone(), to.name.clone()))
        .collect();
    let map = |ty| {
        types.forward.get_with_parameters(
            original.checked_program(),
            candidate.checked_program(),
            ty,
            &parameters,
            &effect_parameters,
        )
    };
    for (a, b) in a
        .params
        .iter()
        .zip(&b.params)
        .chain(std::iter::once((&a.output, &b.output)))
    {
        if map(*a)? != *b {
            return Err(invalid("action payload/result type identity changed"));
        }
    }
    for (a, b) in a.generic_params.iter().zip(&b.generic_params) {
        for (a, b) in a.bounds.iter().zip(&b.bounds) {
            if original
                .native_spec_identity(&a.spec)
                .map_err(Error::Projection)?
                != candidate
                    .native_spec_identity(&b.spec)
                    .map_err(Error::Projection)?
                || a.args.len() != b.args.len()
            {
                return Err(invalid("action spec evidence changed"));
            }
            for (a, b) in a.args.iter().zip(&b.args) {
                if map(*a)? != *b {
                    return Err(invalid("action spec argument identity changed"));
                }
            }
        }
    }
    Ok(())
}

fn invalid(reason: &str) -> Error {
    Error::InvalidRuntimeInterface(reason.into())
}

fn execution_contract(action: &etas_std::EffectActionDecl) -> impl PartialEq + '_ {
    (
        &action.owner,
        &action.name,
        action.stable_id,
        &action.runtime_requirement,
        action.high_impact_ack,
        action.compiler_role,
        &action.default_handler_effects,
        action.default_handler_effect_tail.as_ref().map(|name| {
            action
                .type_params
                .iter()
                .position(|parameter| &parameter.name == name)
                .expect("checked default-handler row binder")
        }),
    )
}
