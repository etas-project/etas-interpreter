use super::InterpValue;
use etas_frontend::CheckedProject;
use etas_types::TypeId;

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
