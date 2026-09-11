use super::{
    host_type_environment::HostTypeEnvironment,
    host_value::host_to_typed_interp_value_with_substitutions,
};
use crate::value::InterpValue;
use etas_host::HostValue;
use etas_types::{TypeId, TypeStore};

#[cfg(test)]
mod tests;

pub(super) fn decode(
    value: HostValue,
    constructor: TypeId,
    args: &[TypeId],
    store: &TypeStore,
    environment: &HostTypeEnvironment<'_>,
) -> Result<InterpValue, String> {
    let layout = environment.enum_layout(constructor)?;
    if args.len() != layout.type_params.len() {
        return Err("checked enum type arguments do not match its layout".into());
    }
    let HostValue::Variant { name, fields } = value else {
        return Err("expected an enum host variant".into());
    };
    let variant = layout
        .variants
        .iter()
        .find(|variant| variant.name == name)
        .ok_or_else(|| {
            format!("variant `{name}` is not a member of checked enum {constructor:?}")
        })?;
    if fields.len() != variant.fields.len() {
        return Err(format!(
            "enum variant `{name}` expects {} fields, received {}",
            variant.fields.len(),
            fields.len()
        ));
    }
    let environment = HostTypeEnvironment::applied(environment, &layout.type_params, args);
    let fields = fields
        .into_iter()
        .zip(&variant.fields)
        .enumerate()
        .map(|(index, (value, ty))| {
            host_to_typed_interp_value_with_substitutions(value, *ty, store, &environment)
                .map_err(|error| format!("enum variant `{name}` field {index}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(InterpValue::Variant { name, fields })
}
