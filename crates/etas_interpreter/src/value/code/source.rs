//! Live compiler capabilities carried by ordinary source definition records.
//! Public descriptions never reconstruct or authorize a compiler definition.
use std::sync::Arc;

use etas_frontend::{CheckedProgram, code::definition::NativeDefinitionStore};
use etas_types::{Type, TypeId};

use super::{AgentDefinitionValue, CallableDefinitionValue};
use crate::value::InterpValue as V;

pub(crate) enum Definition {
    Agent(AgentDefinitionValue),
    Callable(CallableDefinitionValue),
}

pub(crate) fn retained_type(checked: &etas_frontend::CheckedProject) -> Option<TypeId> {
    let known = &checked.types.known_std_types.code;
    let constructor = known.agent_definition.or(known.callable_definition)?;
    retained_field(checked, constructor)
}

pub(crate) fn retained_syntax_type(checked: &etas_frontend::CheckedProject) -> Option<TypeId> {
    retained_field(checked, checked.types.known_std_types.code.syntax_block?)
}

fn retained_field(checked: &etas_frontend::CheckedProject, constructor: TypeId) -> Option<TypeId> {
    let Type::Nominal(owner) = checked.type_store.get(constructor)? else {
        return None;
    };
    let Type::Record(fields) = checked.type_store.get(owner.representation?)? else {
        return None;
    };
    fields
        .fields
        .iter()
        .find(|field| field.name == "retained")
        .map(|field| field.ty)
}

pub(crate) fn syntax_input(
    value: &V,
    checked: &CheckedProgram,
) -> Result<super::SyntaxBlockValue, String> {
    if !checked.hir.source_standard_types {
        return match value {
            V::SyntaxBlock(value) => Ok(value.clone()),
            _ => Err("syntax operation requires a retained AST".into()),
        };
    }
    let V::Nominal { ty, value } = value else {
        return Err("syntax operation requires the selected source SyntaxBlock record".into());
    };
    if checked.types.known_std_types.code.syntax_block != Some(*ty) {
        return Err("syntax operation received a different source record identity".into());
    }
    let V::Record(fields) = value.as_ref() else {
        return Err("source SyntaxBlock has no record representation".into());
    };
    match fields.get("retained") {
        Some(V::SyntaxBlock(value)) if retained_syntax_type(checked) == Some(value.ty()) => {
            Ok(value)
        }
        _ => Err("source SyntaxBlock has no matching private AST capability".into()),
    }
}

impl Definition {
    pub(crate) fn input(value: &V, checked: &CheckedProgram) -> Result<Self, String> {
        let payload = if checked.hir.source_standard_types {
            let V::Nominal { ty, value } = value else {
                return Err("compiler action requires a selected source definition record".into());
            };
            let constructor = match checked.type_store.get(*ty) {
                Some(Type::Applied { constructor, args }) if args.len() == 1 => {
                    TypeId(constructor.0)
                }
                _ => return Err("compiler source definition has no checked target argument".into()),
            };
            let known = &checked.types.known_std_types.code;
            let V::Record(fields) = value.as_ref() else {
                return Err("compiler source definition has no record payload".into());
            };
            let payload = fields
                .get("retained")
                .ok_or("compiler source definition has no retained capability")?;
            let expected = retained_type(checked)
                .ok_or("compiler source definition has no checked retained capability type")?;
            match &payload {
                V::AgentDefinition(handle) if known.agent_definition == Some(constructor)
                    && handle.ty() == expected => {},
                V::CallableDefinition(handle) if known.callable_definition == Some(constructor)
                    && handle.ty() == expected => {},
                _ => return Err("compiler source definition differs from its retained capability family or type".into()),
            }
            payload
        } else {
            value.clone()
        };
        let definition = match payload {
            V::AgentDefinition(value) => Self::Agent(value),
            V::CallableDefinition(value) => Self::Callable(value),
            _ => return Err("compiler action requires its native definition capability".into()),
        };
        if !Arc::ptr_eq(definition.program(), checked) {
            return Err("compiler definition belongs to a different checked program".into());
        }
        // Resolving the reference uses the real retained store, never the
        // record's mutable descriptive identity, signature or revision fields.
        definition
            .backend()
            .resolve(definition.reference()?)
            .map_err(|error| error.to_string())?;
        Ok(definition)
    }

    pub(crate) fn program(&self) -> &CheckedProgram {
        match self {
            Self::Agent(value) => value.program(),
            Self::Callable(value) => value.program(),
        }
    }
    pub(crate) fn backend(&self) -> &Arc<NativeDefinitionStore> {
        match self {
            Self::Agent(value) => value.backend(),
            Self::Callable(value) => value.backend(),
        }
    }
    pub(crate) fn reference(&self) -> Result<&etas_code::definition::DefinitionRef, String> {
        match self {
            Self::Agent(value) => Ok(value.definition()),
            Self::Callable(value) => value.definition(),
        }
    }
}
