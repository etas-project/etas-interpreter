use etas_code::descriptor::{
    AnnotationArgumentInfo, AnnotationConstructorKind, AnnotationLiteralInfo,
    AnnotationReferenceInfo, AnnotationSequenceKind, StaticValueInfo,
};
use etas_types::{Type, TypeId};

use crate::{
    eval::code::project::{Projector, optional},
    value::{CodeMetadataPayload as M, InterpValue as V},
};

pub(in crate::eval::code) fn annotation_value(
    p: &Projector<'_>,
    ty: TypeId,
    value: &StaticValueInfo,
) -> Result<V, String> {
    let (variant, value) = match value {
        StaticValueInfo::Literal(value) => match value {
            AnnotationLiteralInfo::Unit => return p.variant(ty, "Unit", Vec::new()),
            AnnotationLiteralInfo::Bool(value) => ("Bool", V::Bool(*value)),
            AnnotationLiteralInfo::Integer(value) => ("Integer", V::String(value.clone().into())),
            AnnotationLiteralInfo::Float(value) => ("Float", V::String(value.clone().into())),
            AnnotationLiteralInfo::String(value) => ("String", V::String(value.clone().into())),
            AnnotationLiteralInfo::Character(value) => {
                ("Character", V::String(value.to_string().into()))
            }
        },
        StaticValueInfo::Reference(reference) => return reference_value(p, ty, reference),
        StaticValueInfo::Sequence { kind, elements } => {
            let variant = match kind {
                AnnotationSequenceKind::Array => "Array",
                AnnotationSequenceKind::List => "List",
                AnnotationSequenceKind::Set => "Set",
                AnnotationSequenceKind::Tuple => "Tuple",
            };
            let payload = p.variant_payload(ty, variant)?;
            let element_ty = match p.store.get(payload) {
                Some(Type::List(element)) if *kind == AnnotationSequenceKind::List => *element,
                Some(Type::Array(element)) if *kind != AnnotationSequenceKind::List => *element,
                _ => {
                    return Err(
                        "annotation sequence differs from its checked source representation".into(),
                    );
                }
            };
            let values = elements
                .iter()
                .map(|value| annotation_value(p, element_ty, value))
                .collect::<Result<Vec<_>, _>>()?;
            let value = if *kind == AnnotationSequenceKind::List {
                V::List(values.into())
            } else {
                V::Array(values.into())
            };
            (variant, value)
        }
        StaticValueInfo::Record(fields) => {
            let element = p.array_element(p.variant_payload(ty, "Record")?)?;
            let [_, value_ty] = p.tuple_elements(element)? else {
                return Err("annotation record entry is not a checked name/value pair".into());
            };
            let fields = fields
                .iter()
                .map(|field| {
                    Ok(V::Tuple(
                        vec![
                            V::String(field.name.clone().into()),
                            annotation_value(p, *value_ty, &field.value)?,
                        ]
                        .into(),
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            ("Record", V::Array(fields.into()))
        }
        StaticValueInfo::Constructor {
            kind,
            target,
            arguments,
        } => {
            let arguments = annotation_arguments(
                p,
                p.variant_field(ty, "Constructor", "arguments")?,
                arguments,
            )?;
            let target = reference_value(p, p.variant_field(ty, "Constructor", "target")?, target)?;
            return p.named_variant(
                ty,
                "Constructor",
                vec![
                    (
                        "kind",
                        V::String(
                            match kind {
                                AnnotationConstructorKind::Limit => "limit",
                                AnnotationConstructorKind::Trace => "trace",
                            }
                            .into(),
                        ),
                    ),
                    ("target", target),
                    ("arguments", arguments),
                ],
            );
        }
    };
    p.variant(ty, variant, vec![value])
}

fn reference_value(
    p: &Projector<'_>,
    ty: TypeId,
    reference: &AnnotationReferenceInfo,
) -> Result<V, String> {
    match reference {
        AnnotationReferenceInfo::Declaration(key) => p.variant(
            ty,
            "Reference",
            vec![p.metadata(
                p.variant_payload(ty, "Reference")?,
                M::Definition(key.clone()),
            )?],
        ),
        AnnotationReferenceInfo::Module { package, path } => p.named_variant(
            ty,
            "Module",
            vec![
                (
                    "package",
                    V::String(package.fingerprint().to_string().into()),
                ),
                (
                    "path",
                    V::Array(
                        path.iter()
                            .map(|part| V::String(part.clone().into()))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ),
            ],
        ),
    }
}

fn annotation_arguments(
    p: &Projector<'_>,
    ty: TypeId,
    arguments: &[AnnotationArgumentInfo],
) -> Result<V, String> {
    let element = p.array_element(ty)?;
    let value_ty = p.field(element, "value")?;
    arguments
        .iter()
        .map(|argument| {
            p.record(
                element,
                vec![
                    (
                        "name",
                        optional(
                            argument
                                .name
                                .as_ref()
                                .map(|name| V::String(name.clone().into())),
                        ),
                    ),
                    ("value", annotation_value(p, value_ty, &argument.value)?),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|values| V::Array(values.into()))
}
