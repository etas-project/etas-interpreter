use super::InterpValue;
use etas_frontend::CheckedProject;
use etas_types::TypeId;

/// Runtime syntax constructs a value of the checked source enum; its shape is
/// read from the declaration, just as for an ordinary constructor call.
pub(crate) fn construct(
    ty: TypeId,
    name: &str,
    fields: Vec<InterpValue>,
    checked: &CheckedProject,
) -> Result<InterpValue, String> {
    let mut layouts = crate::plan::code::CheckedValueTypes::default();
    let variant = layouts
        .variant(checked, ty, name)
        .map_err(|error| format!("enum constructor has no checked layout: {error:?}"))?;
    if variant.fields.len() != fields.len() {
        return Err("enum constructor differs from its checked payload arity".into());
    }
    Ok(InterpValue::Nominal {
        ty,
        value: InterpValue::Variant {
            name: name.into(),
            fields: fields.into(),
        }
        .into(),
    })
}

/// A projection of a checked enum envelope, never an implicit nominal conversion.
pub(crate) fn payload<'a>(
    value: &'a InterpValue,
    expected: TypeId,
    checked: &CheckedProject,
) -> Result<(&'a str, &'a [InterpValue]), String> {
    let InterpValue::Nominal { ty, value } = value else {
        return Err("enum value is missing its checked owner identity".into());
    };
    if *ty != expected {
        return Err("enum value has a different checked owner identity".into());
    }
    let InterpValue::Variant { name, fields } = value.as_ref() else {
        return Err("checked enum envelope has no constructor payload".into());
    };
    let mut layouts = crate::plan::code::CheckedValueTypes::default();
    let variant = layouts
        .variant(checked, expected, name)
        .map_err(|error| format!("enum value has no checked constructor layout: {error:?}"))?;
    if variant.fields.len() != fields.len() {
        return Err("enum payload differs from its checked constructor arity".into());
    }
    Ok((name, fields))
}
