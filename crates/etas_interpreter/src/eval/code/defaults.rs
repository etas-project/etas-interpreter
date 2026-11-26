use etas_code::{
    CallableId, DefinitionKey,
    descriptor::{ParameterDefaultInfo, ParameterDefaultOwner},
};
use etas_types::TypeId;

use super::project::Projector;
use crate::value::{CodeMetadataPayload as M, InterpValue as V};

pub(super) fn project(
    p: &Projector<'_>,
    ty: TypeId,
    default: &ParameterDefaultInfo,
) -> Result<V, String> {
    let owner_ty = p.field(ty, "owner")?;
    let owner = match &default.owner {
        ParameterDefaultOwner::Declaration(definition) => {
            let layout = p.variant_layout(owner_ty, "Declaration")?;
            let field = *layout
                .fields
                .first()
                .ok_or("default owner has no definition field")?;
            p.variant(
                owner_ty,
                "Declaration",
                vec![p.metadata(field, M::Definition(definition.clone()))?],
            )?
        }
        ParameterDefaultOwner::Method(method) => {
            let layout = p.variant_layout(owner_ty, "Method")?;
            let field = *layout
                .fields
                .first()
                .ok_or("default owner has no method field")?;
            p.variant(
                owner_ty,
                "Method",
                vec![p.metadata(field, M::Callable(CallableId::method(method.clone())))?],
            )?
        }
        ParameterDefaultOwner::SpecMethod { spec, name } => {
            named_owner(p, owner_ty, "SpecMethod", "spec", spec, name)?
        }
        ParameterDefaultOwner::Action { effect, name } => {
            named_owner(p, owner_ty, "Action", "effect", effect, name)?
        }
    };
    p.record(
        ty,
        vec![
            ("injected", V::Bool(default.injected)),
            ("owner", owner),
            (
                "parameter",
                V::usize(
                    usize::try_from(default.parameter)
                        .map_err(|_| "default parameter position exceeds the runtime ABI")?,
                ),
            ),
            ("revision", V::String(default.revision.to_string().into())),
        ],
    )
}

fn named_owner(
    p: &Projector<'_>,
    ty: TypeId,
    variant: &str,
    owner_field: &str,
    definition: &DefinitionKey,
    name: &str,
) -> Result<V, String> {
    let layout = p.variant_layout(ty, variant)?;
    let names = layout
        .field_names
        .as_ref()
        .ok_or("default owner has no named payload layout")?;
    let fields = names
        .iter()
        .zip(&layout.fields)
        .map(|(field, ty)| {
            if field == owner_field {
                p.metadata(*ty, M::Definition(definition.clone()))
            } else if field == "name" {
                Ok(V::String(name.into()))
            } else {
                Err("default owner differs from its checked named payload ABI".into())
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    p.variant(ty, variant, fields)
}
