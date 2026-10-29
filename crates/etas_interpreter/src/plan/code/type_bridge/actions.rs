use std::{collections::BTreeMap, sync::Arc};

use etas_effects::ActionRef;
use etas_frontend::code::definition::{DefinitionCatalog, NativeActionSignature};
use etas_types::TypeId;

use super::{RevisionTypeBridgeError as Error, runtime::RuntimeTypeBridge};

pub(super) struct ActionBridge {
    forward: BTreeMap<ActionRef, Arc<ActionPair>>,
    backward: BTreeMap<ActionRef, Arc<ActionPair>>,
}

#[derive(Debug)]
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
        let targets = target
            .iter()
            .map(|action| ((&action.effect, action.name.as_str()), action))
            .collect::<BTreeMap<_, _>>();
        let mut forward = BTreeMap::new();
        let mut backward = BTreeMap::new();
        for from in source {
            let Some(to) = targets.get(&(&from.effect, from.name.as_str())) else {
                continue;
            };
            validate(original, candidate, &from, to, types)?;
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
        for symbol in original_program.std_registry.symbols() {
            let etas_std::StdDecl::EffectAction(declaration) = &symbol.decl else {
                continue;
            };
            let Some(id) = declaration.stable_id else {
                continue;
            };
            let Some(other) = candidate_program
                .std_registry
                .lookup_qualified(&symbol.qualified_path)
            else {
                continue;
            };
            if other.decl != symbol.decl {
                return Err(invalid(
                    "standard action declaration changed across the frozen environment",
                ));
            }
            let id = etas_effects::EffectActionId(id);
            let (Some(from), Some(to)) = (
                original_program.effect_registry.standard_action(id),
                candidate_program.effect_registry.standard_action(id),
            ) else {
                continue;
            };
            if original
                .native_effect_identity(from.owner)
                .map_err(Error::Projection)?
                != candidate
                    .native_effect_identity(to.owner)
                    .map_err(Error::Projection)?
                || from.name != to.name
            {
                return Err(invalid(
                    "standard action identity changed across the frozen environment",
                ));
            }
            let from = ActionRef {
                tag: from.owner,
                action: from.id,
            };
            let to = ActionRef {
                tag: to.owner,
                action: to.id,
            };
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
        self.backward
            .get(action)
            .ok_or_else(|| invalid("action has no checked original declaration"))
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
    let map = |ty| {
        types.forward.get_with_parameters(
            original.checked_program(),
            candidate.checked_program(),
            ty,
            &parameters,
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
