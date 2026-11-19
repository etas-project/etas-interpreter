use etas_code::descriptor::{
    EffectInfo, EffectRowInfoRef, EffectTargetInfo, GenericParameterRef, StaticEffectArgInfo,
};
use etas_types::TypeId;

use super::Description;
use crate::{
    eval::code::project::optional,
    value::{CodeMetadataPayload as M, InterpValue as V},
};

impl Description<'_, '_, '_> {
    pub(in crate::eval::code) fn row(
        &mut self,
        ty: TypeId,
        root: EffectRowInfoRef,
        actions: bool,
    ) -> Result<V, String> {
        let row = self
            .graph
            .effect_row(root)
            .ok_or("effect row description is missing")?
            .clone();
        let field = if actions { "actions" } else { "effects" };
        let element = self.p.array_element(self.p.field(ty, field)?)?;
        let effects = row
            .effects
            .iter()
            .map(|effect| self.effect(element, effect))
            .collect::<Result<Vec<_>, _>>()?;
        let tail_ty = self.p.option(self.p.field(ty, "tail")?)?;
        let tail = row
            .tail
            .map(|tail| self.parameter(tail_ty, tail))
            .transpose()?;
        self.p.record(
            ty,
            vec![(field, V::Array(effects.into())), ("tail", optional(tail))],
        )
    }

    pub(in crate::eval::code) fn parameter(
        &self,
        ty: TypeId,
        root: GenericParameterRef,
    ) -> Result<V, String> {
        let parameter = self
            .graph
            .binder(root)
            .ok_or("generic parameter description is missing")?;
        self.p.record(
            ty,
            vec![
                (
                    "owner",
                    self.p.metadata(
                        self.p.field(ty, "owner")?,
                        M::GenericOwner(parameter.owner.clone()),
                    )?,
                ),
                ("position", V::usize(parameter.position as usize)),
            ],
        )
    }

    pub(in crate::eval::code) fn effect(
        &mut self,
        ty: TypeId,
        effect: &EffectInfo,
    ) -> Result<V, String> {
        let target = self.target(self.p.field(ty, "target")?, &effect.target)?;
        let element = self.p.array_element(self.p.field(ty, "arguments")?)?;
        let arguments = effect
            .arguments
            .iter()
            .map(|arg| self.effect_argument(element, arg))
            .collect::<Result<Vec<_>, _>>()?;
        self.p.record(
            ty,
            vec![
                ("target", target),
                ("arguments", V::Array(arguments.into())),
            ],
        )
    }

    pub(in crate::eval::code) fn target(
        &self,
        ty: TypeId,
        target: &EffectTargetInfo,
    ) -> Result<V, String> {
        effect_target(self.p, ty, target)
    }

    fn effect_argument(&mut self, ty: TypeId, arg: &StaticEffectArgInfo) -> Result<V, String> {
        let (name, value) = match arg {
            StaticEffectArgInfo::Type(root) => (
                "Type",
                self.type_info(self.p.variant_payload(ty, "Type")?, *root)?,
            ),
            StaticEffectArgInfo::Resource(key) => (
                "Reference",
                self.p.metadata(
                    self.p.variant_payload(ty, "Reference")?,
                    M::Definition(key.clone()),
                )?,
            ),
            StaticEffectArgInfo::Callable(key) => (
                "Callable",
                self.p.metadata(
                    self.p.variant_payload(ty, "Callable")?,
                    M::Callable(key.clone()),
                )?,
            ),
            StaticEffectArgInfo::String(value) => ("String", V::String(value.clone().into())),
            StaticEffectArgInfo::Integer(value) => ("Integer", V::String(value.clone().into())),
            StaticEffectArgInfo::Wildcard => return self.p.variant(ty, "Wildcard", Vec::new()),
            StaticEffectArgInfo::ResourceProjection { root, fields } => {
                return self.p.named_variant(
                    ty,
                    "Resource",
                    vec![
                        (
                            "root",
                            self.p.metadata(
                                self.p.variant_field(ty, "Resource", "root")?,
                                M::Definition(root.clone()),
                            )?,
                        ),
                        (
                            "fields",
                            V::Array(
                                fields
                                    .iter()
                                    .map(|field| V::String(field.clone().into()))
                                    .collect::<Vec<_>>()
                                    .into(),
                            ),
                        ),
                    ],
                );
            }
        };
        self.p.variant(ty, name, vec![value])
    }
}

pub(super) fn effect_target(
    p: &crate::eval::code::project::Projector<'_>,
    ty: TypeId,
    target: &EffectTargetInfo,
) -> Result<V, String> {
    match target {
        EffectTargetInfo::Effect(key) => p.variant(
            ty,
            "Effect",
            vec![p.metadata(p.variant_payload(ty, "Effect")?, M::Definition(key.clone()))?],
        ),
        EffectTargetInfo::Action { effect, name } => p.named_variant(
            ty,
            "Action",
            vec![
                (
                    "effect",
                    p.metadata(
                        p.variant_field(ty, "Action", "effect")?,
                        M::Definition(effect.clone()),
                    )?,
                ),
                ("name", V::String(name.clone().into())),
            ],
        ),
    }
}
