use etas_code::{
    CallableId, DefinitionKey,
    descriptor::{
        BuiltinTypeInfo, FieldInfo, GenericParameterKind, GenericParameterRef, PrimitiveInfo,
        SignatureInfo, SpecInfo, TypeInfo, TypeInfoRef, VariantPayloadInfo,
    },
};
use etas_types::TypeId;

use super::Description;
use crate::{
    eval::code::project::optional,
    value::{CodeMetadataPayload as M, InterpValue as V},
};

impl Description<'_, '_, '_> {
    pub(in crate::eval::code) fn type_info(
        &mut self,
        ty: TypeId,
        root: TypeInfoRef,
    ) -> Result<V, String> {
        let node = self
            .graph
            .type_info(root)
            .ok_or("type description is missing")?
            .clone();
        if let TypeInfo::Applied {
            constructor,
            arguments,
        } = &node
        {
            if self.expanding.contains(constructor) {
                if let Some(key) = self.named_definition(*constructor) {
                    return self.reference(ty, &key, arguments);
                }
            }
        }
        let named = self.named_definition(root);
        if let Some(key) = &named {
            if !self.expanding.insert(root) {
                return self.reference(ty, key, &[]);
            }
        }
        let result = self.type_node(ty, &node);
        if named.is_some() {
            self.expanding.remove(&root);
        }
        result
    }

    fn named_definition(&self, root: TypeInfoRef) -> Option<DefinitionKey> {
        match self.graph.type_info(root)? {
            TypeInfo::Nominal { definition, .. }
            | TypeInfo::Alias { definition, .. }
            | TypeInfo::Enum { definition, .. } => Some(definition.clone()),
            _ => None,
        }
    }

    fn reference(
        &mut self,
        ty: TypeId,
        key: &DefinitionKey,
        arguments: &[TypeInfoRef],
    ) -> Result<V, String> {
        let p = self.p;
        let args =
            self.type_arguments(p.variant_field(ty, "Reference", "arguments")?, arguments)?;
        p.named_variant(
            ty,
            "Reference",
            vec![
                (
                    "definition",
                    p.metadata(
                        p.variant_field(ty, "Reference", "definition")?,
                        M::Definition(key.clone()),
                    )?,
                ),
                ("arguments", args),
            ],
        )
    }

    fn type_node(&mut self, ty: TypeId, node: &TypeInfo) -> Result<V, String> {
        let p = self.p;
        match node {
            TypeInfo::Primitive(value) => {
                p.variant(ty, "Primitive", vec![V::String(primitive(*value).into())])
            }
            TypeInfo::Builtin {
                constructor,
                arguments,
            } => {
                let args =
                    self.type_array(p.variant_field(ty, "Builtin", "arguments")?, arguments)?;
                p.named_variant(
                    ty,
                    "Builtin",
                    vec![
                        ("name", V::String(builtin(*constructor).into())),
                        ("arguments", args),
                    ],
                )
            }
            TypeInfo::Parameter(root) => p.variant(
                ty,
                "Parameter",
                vec![self.parameter(p.variant_payload(ty, "Parameter")?, *root)?],
            ),
            TypeInfo::Tuple(elements) => {
                let elements = self.type_array(p.variant_payload(ty, "Tuple")?, elements)?;
                p.variant(ty, "Tuple", vec![elements])
            }
            TypeInfo::Record(fields) => {
                let fields = self.fields(p.variant_payload(ty, "Record")?, fields)?;
                p.variant(ty, "Record", vec![fields])
            }
            TypeInfo::Nominal {
                definition,
                parameters,
                representation,
            } => {
                let repr_ty = p.option(p.variant_field(ty, "Nominal", "representation")?)?;
                let repr = representation
                    .map(|root| self.type_info(repr_ty, root))
                    .transpose()?;
                let mut fields = self.named_fields(ty, "Nominal", definition, parameters)?;
                fields.push(("representation", optional(repr)));
                p.named_variant(ty, "Nominal", fields)
            }
            TypeInfo::Alias {
                definition,
                parameters,
                target,
            } => {
                let target = self.type_info(p.variant_field(ty, "Alias", "target")?, *target)?;
                let mut fields = self.named_fields(ty, "Alias", definition, parameters)?;
                fields.push(("target", target));
                p.named_variant(ty, "Alias", fields)
            }
            TypeInfo::Enum {
                definition,
                parameters,
                variants,
            } => {
                let element = p.array_element(p.variant_field(ty, "Enum", "variants")?)?;
                let mut values = Vec::with_capacity(variants.len());
                for variant in variants {
                    let payload =
                        self.variant_payload(p.field(element, "payload")?, &variant.payload)?;
                    values.push(p.record(
                        element,
                        vec![
                            ("name", V::String(variant.name.clone().into())),
                            ("payload", payload),
                        ],
                    )?);
                }
                let mut fields = self.named_fields(ty, "Enum", definition, parameters)?;
                fields.push(("variants", V::Array(values.into())));
                p.named_variant(ty, "Enum", fields)
            }
            TypeInfo::Applied {
                constructor,
                arguments,
            } => {
                let constructor =
                    self.type_info(p.variant_field(ty, "Applied", "constructor")?, *constructor)?;
                let arguments =
                    self.type_arguments(p.variant_field(ty, "Applied", "arguments")?, arguments)?;
                p.named_variant(
                    ty,
                    "Applied",
                    vec![("constructor", constructor), ("arguments", arguments)],
                )
            }
            TypeInfo::Function(signature) => {
                let signature = self.signature(p.variant_payload(ty, "Function")?, signature)?;
                p.variant(ty, "Function", vec![signature])
            }
            TypeInfo::Handler {
                handled,
                produced,
                result,
            } => {
                let handled =
                    self.row(p.variant_field(ty, "Handler", "handled")?, *handled, false)?;
                let produced_ty = p.option(p.variant_field(ty, "Handler", "produced")?)?;
                let produced = produced
                    .map(|root| self.row(produced_ty, root, false))
                    .transpose()?;
                let result_ty = p.option(p.variant_field(ty, "Handler", "result")?)?;
                let result = result
                    .map(|root| self.type_info(result_ty, root))
                    .transpose()?;
                p.named_variant(
                    ty,
                    "Handler",
                    vec![
                        ("handled", handled),
                        ("produced", optional(produced)),
                        ("result", optional(result)),
                    ],
                )
            }
            TypeInfo::Existential { bounds } => {
                let element = p.array_element(p.variant_payload(ty, "Existential")?)?;
                let bounds = bounds
                    .iter()
                    .map(|bound| self.spec(element, bound))
                    .collect::<Result<Vec<_>, _>>()?;
                p.variant(ty, "Existential", vec![V::Array(bounds.into())])
            }
            TypeInfo::AgentContext(method) => p.variant(
                ty,
                "AgentContext",
                vec![p.metadata(
                    p.variant_payload(ty, "AgentContext")?,
                    M::Callable(CallableId::method(method.clone())),
                )?],
            ),
            TypeInfo::Task { output, effects } => {
                let output = self.type_info(p.variant_field(ty, "Task", "output")?, *output)?;
                let effects = self.row(p.variant_field(ty, "Task", "effects")?, *effects, false)?;
                p.named_variant(ty, "Task", vec![("output", output), ("effects", effects)])
            }
        }
    }

    fn named_fields(
        &mut self,
        ty: TypeId,
        variant: &str,
        definition: &DefinitionKey,
        parameters: &[GenericParameterRef],
    ) -> Result<Vec<(&'static str, V)>, String> {
        let p = self.p;
        let element = p.array_element(p.variant_field(ty, variant, "parameters")?)?;
        let parameters = parameters
            .iter()
            .map(|root| self.parameter(element, *root))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(vec![
            (
                "definition",
                p.metadata(
                    p.variant_field(ty, variant, "definition")?,
                    M::Definition(definition.clone()),
                )?,
            ),
            ("parameters", V::Array(parameters.into())),
        ])
    }

    fn type_arguments(&mut self, ty: TypeId, arguments: &[TypeInfoRef]) -> Result<V, String> {
        let p = self.p;
        let element = p.array_element(ty)?;
        let payload = p.variant_payload(element, "Type")?;
        let arguments = arguments
            .iter()
            .map(|root| {
                let value = self.type_info(payload, *root)?;
                p.variant(element, "Type", vec![value])
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(V::Array(arguments.into()))
    }

    pub(in crate::eval::code) fn type_array(
        &mut self,
        ty: TypeId,
        roots: &[TypeInfoRef],
    ) -> Result<V, String> {
        let element = self.p.array_element(ty)?;
        roots
            .iter()
            .map(|root| self.type_info(element, *root))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| V::Array(values.into()))
    }

    fn fields(&mut self, ty: TypeId, fields: &[FieldInfo]) -> Result<V, String> {
        let p = self.p;
        let element = p.array_element(ty)?;
        let field_ty = p.field(element, "ty")?;
        let fields = fields
            .iter()
            .map(|field| {
                let value = self.type_info(field_ty, field.ty)?;
                p.record(
                    element,
                    vec![
                        ("name", V::String(field.name.clone().into())),
                        ("ty", value),
                    ],
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(V::Array(fields.into()))
    }

    fn variant_payload(&mut self, ty: TypeId, payload: &VariantPayloadInfo) -> Result<V, String> {
        let p = self.p;
        let (name, fields) = match payload {
            VariantPayloadInfo::None => return p.variant(ty, "None", Vec::new()),
            VariantPayloadInfo::Positional(roots) => (
                "Positional",
                self.type_array(p.variant_payload(ty, "Positional")?, roots)?,
            ),
            VariantPayloadInfo::Named(fields) => (
                "Named",
                self.fields(p.variant_payload(ty, "Named")?, fields)?,
            ),
        };
        p.variant(ty, name, vec![fields])
    }

    pub(in crate::eval::code) fn spec(&mut self, ty: TypeId, spec: &SpecInfo) -> Result<V, String> {
        let p = self.p;
        let args = self.type_array(p.field(ty, "arguments")?, &spec.arguments)?;
        p.record(
            ty,
            vec![
                (
                    "definition",
                    p.optional_metadata(
                        p.field(ty, "definition")?,
                        Some(M::Definition(spec.definition.clone())),
                    )?,
                ),
                ("arguments", args),
                ("trace", optional(None)),
            ],
        )
    }

    pub(in crate::eval::code) fn signature(
        &mut self,
        ty: TypeId,
        signature: &SignatureInfo,
    ) -> Result<V, String> {
        let p = self.p;
        let parameter_ty = p.array_element(p.field(ty, "parameters")?)?;
        let mut parameters = Vec::with_capacity(signature.parameters.len());
        for parameter in &signature.parameters {
            let value = self.type_info(p.field(parameter_ty, "ty")?, parameter.ty)?;
            let default_ty = p.option(p.field(parameter_ty, "default")?)?;
            let default = parameter
                .default
                .as_ref()
                .map(|default| crate::eval::code::defaults::project(p, default_ty, default))
                .transpose()?;
            parameters.push(p.record(
                parameter_ty,
                vec![
                        (
                            "name",
                            optional(
                                parameter
                                    .name
                                    .as_ref()
                                    .map(|name| V::String(name.clone().into())),
                            ),
                        ),
                        ("ty", value),
                        ("default", optional(default)),
                    ],
            )?);
        }
        let generic_ty = p.array_element(p.field(ty, "generics")?)?;
        let bound_ty = p.array_element(p.field(generic_ty, "bounds")?)?;
        let mut generics = Vec::with_capacity(signature.generics.len());
        for root in &signature.generics {
            let binder = self
                .graph
                .binder(*root)
                .ok_or("signature generic description is missing")?
                .clone();
            let bounds = binder
                .bounds
                .iter()
                .map(|bound| self.spec(bound_ty, bound))
                .collect::<Result<Vec<_>, _>>()?;
            generics.push(p.record(
                generic_ty,
                vec![
                        (
                            "owner",
                            p.metadata(
                                p.field(generic_ty, "owner")?,
                                M::GenericOwner(binder.owner),
                            )?,
                        ),
                        ("position", V::usize(binder.position as usize)),
                        ("name", V::String(binder.name.into())),
                        (
                            "kind",
                            V::String(
                                match binder.kind {
                                    GenericParameterKind::Type => "type",
                                    GenericParameterKind::EffectRow => "effect",
                                }
                                .into(),
                            ),
                        ),
                        ("bounds", V::Array(bounds.into())),
                    ],
            )?);
        }
        let output = self.type_info(p.field(ty, "output")?, signature.output)?;
        let declared_ty = p.option(p.field(ty, "declared_output")?)?;
        let declared = signature
            .declared_output
            .map(|root| self.type_info(declared_ty, root))
            .transpose()?;
        let effect_ty = p.option(p.field(ty, "effects")?)?;
        let effects = signature
            .effects
            .map(|root| self.row(effect_ty, root, false))
            .transpose()?;
        p.record(
            ty,
            vec![
                ("generics", V::Array(generics.into())),
                ("parameters", V::Array(parameters.into())),
                ("output", output),
                ("declared_output", optional(declared)),
                ("effects", optional(effects)),
            ],
        )
    }
}

fn primitive(value: PrimitiveInfo) -> &'static str {
    match value {
        PrimitiveInfo::Unit => "unit",
        PrimitiveInfo::Never => "never",
        PrimitiveInfo::Bool => "bool",
        PrimitiveInfo::I8 => "i8",
        PrimitiveInfo::I16 => "i16",
        PrimitiveInfo::I32 => "i32",
        PrimitiveInfo::I64 => "i64",
        PrimitiveInfo::I128 => "i128",
        PrimitiveInfo::Isize => "isize",
        PrimitiveInfo::U8 => "u8",
        PrimitiveInfo::U16 => "u16",
        PrimitiveInfo::U32 => "u32",
        PrimitiveInfo::U64 => "u64",
        PrimitiveInfo::U128 => "u128",
        PrimitiveInfo::Usize => "usize",
        PrimitiveInfo::F32 => "f32",
        PrimitiveInfo::F64 => "f64",
        PrimitiveInfo::Char => "char",
        PrimitiveInfo::String => "string",
        PrimitiveInfo::Bytes => "bytes",
    }
}

fn builtin(value: BuiltinTypeInfo) -> &'static str {
    match value {
        BuiltinTypeInfo::Array => "Array",
        BuiltinTypeInfo::List => "List",
        BuiltinTypeInfo::Map => "Map",
        BuiltinTypeInfo::Set => "Set",
        BuiltinTypeInfo::Range => "Range",
        BuiltinTypeInfo::Slice => "Slice",
        BuiltinTypeInfo::Option => "Option",
        BuiltinTypeInfo::Result => "Result",
        BuiltinTypeInfo::Schema => "Schema",
        BuiltinTypeInfo::Prompt => "Prompt",
        BuiltinTypeInfo::PromptPart => "PromptPart",
        BuiltinTypeInfo::Message => "Message",
        BuiltinTypeInfo::MemorySelection => "MemorySelection",
        BuiltinTypeInfo::Store => "Store",
        BuiltinTypeInfo::MemoryRegion => "MemoryRegion",
        BuiltinTypeInfo::MemoryRegionHandle => "MemoryRegionHandle",
        BuiltinTypeInfo::ExternalToolHandle => "ExternalToolHandle",
        BuiltinTypeInfo::Trusted => "Trusted",
        BuiltinTypeInfo::Untrusted => "Untrusted",
        BuiltinTypeInfo::Secret => "Secret",
        BuiltinTypeInfo::Public => "Public",
        BuiltinTypeInfo::Sanitized => "Sanitized",
        BuiltinTypeInfo::CallableDefinition => "CallableDefinition",
        BuiltinTypeInfo::SchedulerHandle => "SchedulerHandle",
        BuiltinTypeInfo::Continuation => "Continuation",
    }
}
