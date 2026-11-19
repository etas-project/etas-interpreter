//! Shape of a compiler-derived decoder for an installed source Schema<T>.
use super::InterpValue;
use etas_frontend::CheckedProject;
use etas_types::{FlowType, Type, TypeId};

pub(crate) struct DecoderSignature<'a> {
    pub output: TypeId,
    pub error: TypeId,
    pub signature: &'a FlowType,
}

fn field(program: &CheckedProject, owner: TypeId, name: &str) -> Option<TypeId> {
    let representation =
        etas_types::ty::applied_representation_in_store(&program.type_store, owner).ok()??;
    let Type::Record(record) = program.type_store.get(representation)? else {
        return None;
    };
    record
        .fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.ty)
}

pub(crate) fn layout(program: &CheckedProject, schema: TypeId) -> Result<(TypeId, TypeId), String> {
    let decoder =
        field(program, schema, "decoder").ok_or("checked Schema has no decoder record")?;
    let callable =
        field(program, decoder, "decode").ok_or("checked ResponseDecode has no decode field")?;
    Ok((decoder, callable))
}

pub(crate) fn decoder_signature(
    program: &CheckedProject,
    callable: TypeId,
) -> Result<DecoderSignature<'_>, String> {
    let Some(Type::Function(signature)) = program.type_store.get(callable) else {
        return Err("derived decoder has no checked callable signature".into());
    };
    let (output, error) = etas_types::value_representation::result_parameters(
        &program.type_store,
        &program.types.known_std_types,
        signature.output,
    )
    .ok_or("derived decoder has no checked Result type")?;
    let response =
        crate::eval::resolve_std_type(program, &["std", "agent", "schema", "ModelResponse"]);
    let schema_error =
        crate::eval::resolve_std_type(program, &["std", "runtime", "error", "SchemaError"]);
    if let [input] = signature.input.as_slice()
        && Some(*input) == response
        && Some(error) == schema_error
        && signature
            .effects
            .as_ref()
            .is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
    {
        Ok(DecoderSignature {
            output,
            error,
            signature,
        })
    } else {
        Err("derived decoder disagrees with the checked ResponseDecode signature".into())
    }
}

pub(crate) fn matches_output(
    checked: &CheckedProject,
    value: &InterpValue,
    output: TypeId,
) -> bool {
    match value {
        InterpValue::Schema(actual) => *actual == output,
        InterpValue::Nominal { ty, .. } => matches!(checked.type_store.get(*ty),
          Some(Type::Applied {constructor,args}) if args.as_slice()==[output]
            && crate::eval::resolve_std_type(checked,&["std","agent","schema","Schema"]) == Some(TypeId(constructor.0))),
        _ => false,
    }
}
