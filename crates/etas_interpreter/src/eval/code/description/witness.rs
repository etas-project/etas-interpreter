use etas_code::contract::{
    CanonicalEffect, CanonicalSelector, HandlerControlLabel, HandlerScopeLabel,
    TraceInclusionWitness, TraceLabel,
};
use etas_types::TypeId;

use super::{effects::effect_target, observations};
use crate::{
    eval::code::project::{Projector, optional},
    value::{CodeMetadataPayload as M, InterpValue as V},
};

pub(in crate::eval::code) fn witness(
    p: &Projector<'_>,
    ty: TypeId,
    witness: &TraceInclusionWitness,
) -> Result<V, String> {
    let element = p.array_element(p.field(ty, "path")?)?;
    let label_ty = p.field(element, "label")?;
    let path = witness
        .path
        .iter()
        .map(|(root, value)| {
            p.record(
                element,
                vec![
                    ("descriptor_position", V::usize(root.0 as usize)),
                    ("label", label(p, label_ty, value)?),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    p.record(ty, vec![("path", V::Array(path.into()))])
}

fn label(p: &Projector<'_>, ty: TypeId, label: &TraceLabel) -> Result<V, String> {
    match label {
        TraceLabel::Action(value) => p.variant(
            ty,
            "Action",
            vec![effect(p, p.variant_payload(ty, "Action")?, value)?],
        ),
        TraceLabel::ParameterCall {
            callable,
            position,
            projection,
            signature,
            arguments,
            handlers,
        }
        | TraceLabel::ParameterNonReturn {
            callable,
            position,
            projection,
            signature,
            arguments,
            handlers,
        } => {
            let variant = if matches!(label, TraceLabel::ParameterCall { .. }) {
                "ParameterCall"
            } else {
                "ParameterNonReturn"
            };
            let args = observations::arguments(
                p,
                p.variant_field(ty, variant, "arguments")?,
                arguments.as_deref(),
            )?;
            let handlers_ty = p.option(p.variant_field(ty, variant, "handlers")?)?;
            let handler_element = p.array_element(handlers_ty)?;
            let handlers = handlers
                .as_ref()
                .map(|handlers| {
                    handlers
                        .iter()
                        .map(|handler| handler_scope(p, handler_element, handler))
                        .collect::<Result<Vec<_>, _>>()
                        .map(|values| V::Array(values.into()))
                })
                .transpose()?;
            p.named_variant(
                ty,
                variant,
                vec![
                    (
                        "callable",
                        p.metadata(
                            p.variant_field(ty, variant, "callable")?,
                            M::Callable(callable.clone()),
                        )?,
                    ),
                    ("position", V::usize(*position as usize)),
                    (
                        "projection",
                        super::value_projection::path(
                            p,
                            p.variant_field(ty, variant, "projection")?,
                            projection,
                        )?,
                    ),
                    ("signature", V::String(signature.to_string().into())),
                    ("arguments", args),
                    ("handlers", optional(handlers)),
                ],
            )
        }
        TraceLabel::Returned {
            ty: value,
            observation,
        } => p.named_variant(
            ty,
            "Returned",
            vec![
                ("ty", V::String(value.to_string().into())),
                (
                    "observation",
                    observations::observation(
                        p,
                        p.variant_field(ty, "Returned", "observation")?,
                        observation,
                    )?,
                ),
            ],
        ),
        TraceLabel::Raised { error } => p.named_variant(
            ty,
            "Raised",
            vec![("error", V::String(error.to_string().into()))],
        ),
        TraceLabel::NonReturn => p.variant(ty, "NonReturn", Vec::new()),
    }
}

fn handler_scope(p: &Projector<'_>, ty: TypeId, scope: &HandlerScopeLabel) -> Result<V, String> {
    let HandlerScopeLabel::Handle(arms) = scope;
    let element = p.array_element(p.variant_payload(ty, "Handle")?)?;
    let mut values = Vec::with_capacity(arms.len());
    for arm in arms {
        let action = effect(p, p.field(element, "action")?, &arm.action)?;
        let control = p.variant(
            p.field(element, "control")?,
            match arm.control {
                HandlerControlLabel::Resume => "Resume",
                HandlerControlLabel::Finish => "Finish",
            },
            Vec::new(),
        )?;
        let value = observations::observation(p, p.field(element, "value")?, &arm.value)?;
        values.push(p.record(
            element,
            vec![
                ("action", action),
                ("trace", V::String(arm.trace.to_string().into())),
                ("control", control),
                ("result_type", V::String(arm.result_type.to_string().into())),
                ("value", value),
            ],
        )?);
    }
    p.variant(ty, "Handle", vec![V::Array(values.into())])
}

fn effect(p: &Projector<'_>, ty: TypeId, effect: &CanonicalEffect) -> Result<V, String> {
    let target = effect_target(p, p.field(ty, "target")?, &effect.target)?;
    let element = p.array_element(p.field(ty, "arguments")?)?;
    let arguments = effect
        .arguments
        .iter()
        .map(|arg| selector(p, element, arg))
        .collect::<Result<Vec<_>, _>>()?;
    p.record(
        ty,
        vec![
            ("target", target),
            ("arguments", V::Array(arguments.into())),
        ],
    )
}

fn selector(p: &Projector<'_>, ty: TypeId, value: &CanonicalSelector) -> Result<V, String> {
    let (variant, value) = match value {
        CanonicalSelector::Type(value) => ("TypeFingerprint", V::String(value.to_string().into())),
        CanonicalSelector::Resource(key) => (
            "Reference",
            p.metadata(
                p.variant_payload(ty, "Reference")?,
                M::Definition(key.clone()),
            )?,
        ),
        CanonicalSelector::Callable(key) => (
            "Callable",
            p.metadata(p.variant_payload(ty, "Callable")?, M::Callable(key.clone()))?,
        ),
        CanonicalSelector::String(value) => ("String", V::String(value.clone().into())),
        CanonicalSelector::Integer(value) => ("Integer", V::String(value.clone().into())),
        CanonicalSelector::Wildcard => return p.variant(ty, "Wildcard", Vec::new()),
        CanonicalSelector::ResourceProjection { root, fields } => {
            return p.named_variant(
                ty,
                "Resource",
                vec![
                    (
                        "root",
                        p.metadata(
                            p.variant_field(ty, "Resource", "root")?,
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
    p.variant(ty, variant, vec![value])
}
