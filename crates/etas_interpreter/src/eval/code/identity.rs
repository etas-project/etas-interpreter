use etas_code::{
    CallableId, DefinitionKey, DefinitionKind, ImplementationKey, descriptor::GenericOwner,
    identity::CallableIdentityRef,
};
use etas_types::TypeId;

use super::project::{Projector, optional};
use crate::value::InterpValue as V;

pub(super) fn definition(p: &Projector<'_>, ty: TypeId, key: &DefinitionKey) -> Result<V, String> {
    let kind = match key.kind() {
        DefinitionKind::Agent => "Agent",
        DefinitionKind::Flow => "Flow",
        DefinitionKind::Tool => "Tool",
        DefinitionKind::Type => "Type",
        DefinitionKind::Effect => "Effect",
        DefinitionKind::Spec => "Spec",
        DefinitionKind::Value => "Value",
        DefinitionKind::EnumVariant => "EnumVariant",
        DefinitionKind::Requirement => "Requirement",
        DefinitionKind::Protocol => "Protocol",
    };
    p.record(
        ty,
        vec![
            (
                "package",
                V::String(key.package().fingerprint().to_string().into()),
            ),
            ("module", path(key.module())),
            ("declaration", path(key.declaration())),
            ("kind", p.variant(p.field(ty, "kind")?, kind, Vec::new())?),
        ],
    )
}

fn implementation(p: &Projector<'_>, ty: TypeId, key: &ImplementationKey) -> Result<V, String> {
    let spec_ty = p.option(p.field(ty, "selected_spec")?)?;
    p.record(
        ty,
        vec![
            (
                "package",
                V::String(key.package().fingerprint().to_string().into()),
            ),
            ("module", path(key.module())),
            (
                "subject",
                definition(p, p.field(ty, "subject")?, key.subject())?,
            ),
            (
                "selected_spec",
                optional(
                    key.selected_spec()
                        .map(|spec| definition(p, spec_ty, spec))
                        .transpose()?,
                ),
            ),
            ("header", V::String(key.header().to_string().into())),
        ],
    )
}

pub(super) fn callable(p: &Projector<'_>, ty: TypeId, key: &CallableId) -> Result<V, String> {
    match key.identity_ref() {
        CallableIdentityRef::Declaration(key) => {
            let field = tuple_field(p, ty, "Declaration")?;
            p.variant(ty, "Declaration", vec![definition(p, field, key)?])
        }
        CallableIdentityRef::Method(method) => {
            let owner = method.implementation();
            let implementation_ty = p.option(p.variant_field(ty, "Method", "implementation")?)?;
            p.named_variant(
                ty,
                "Method",
                vec![
                    (
                        "owner",
                        definition(p, p.variant_field(ty, "Method", "owner")?, owner.subject())?,
                    ),
                    (
                        "implementation",
                        optional(Some(implementation(p, implementation_ty, owner)?)),
                    ),
                    ("name", V::String(method.name().into())),
                    (
                        "overload",
                        optional(
                            method
                                .overload()
                                .map(|signature| V::String(signature.to_string().into())),
                        ),
                    ),
                ],
            )
        }
    }
}

pub(super) fn generic_owner(
    p: &Projector<'_>,
    ty: TypeId,
    owner: &GenericOwner,
) -> Result<V, String> {
    let (variant, value) = match owner {
        GenericOwner::Declaration(key) => (
            "Declaration",
            definition(p, tuple_field(p, ty, "Declaration")?, key)?,
        ),
        GenericOwner::Method(method) => (
            "Method",
            callable(
                p,
                tuple_field(p, ty, "Method")?,
                &CallableId::method(method.clone()),
            )?,
        ),
        GenericOwner::Implementation(key) => (
            "Implementation",
            implementation(p, tuple_field(p, ty, "Implementation")?, key)?,
        ),
        GenericOwner::ImplementationPattern(key) => (
            "ImplementationPattern",
            definition(p, tuple_field(p, ty, "ImplementationPattern")?, key)?,
        ),
        GenericOwner::Action { effect, name } => {
            return p.named_variant(
                ty,
                "Action",
                vec![
                    (
                        "effect",
                        definition(p, p.variant_field(ty, "Action", "effect")?, effect)?,
                    ),
                    ("name", V::String(name.as_str().into())),
                ],
            );
        }
        GenericOwner::ActionOverload {
            effect,
            name,
            signature,
        } => {
            return p.named_variant(
                ty,
                "ActionOverload",
                vec![
                    (
                        "effect",
                        definition(p, p.variant_field(ty, "ActionOverload", "effect")?, effect)?,
                    ),
                    ("name", V::String(name.as_str().into())),
                    ("signature", V::String(signature.to_string().into())),
                ],
            );
        }
    };
    p.variant(ty, variant, vec![value])
}

fn tuple_field(p: &Projector<'_>, ty: TypeId, variant: &str) -> Result<TypeId, String> {
    p.variant_layout(ty, variant)?
        .fields
        .first()
        .copied()
        .ok_or_else(|| format!("compiler enum projection `{variant}` has no tuple field"))
}

fn path(segments: &[String]) -> V {
    V::Array(
        segments
            .iter()
            .map(|segment| V::String(segment.clone().into()))
            .collect::<Vec<_>>()
            .into(),
    )
}
