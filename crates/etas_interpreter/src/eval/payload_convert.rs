//! Type-directed neutral representation conversion. Message metadata and
//! conversion policy remain in the source Message.cast method.
use super::*;
use etas_host::HostErrorCode;
use etas_types::{Type, TypeId};

impl EvalContext<'_> {
    pub(super) fn execute_payload_conversion(
        &self,
        item: HirItemId,
        args: &[InterpValue],
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> ControlSignal {
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            unreachable!("checked payload conversion signature")
        };
        let [value] = args else {
            return ControlSignal::missing_checked_fact(
                "payload conversion requires its checked source value",
                span,
            );
        };
        let input = match self.materialized_type(signature.params[0], bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let output = match self.materialized_type(signature.output, bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let target = etas_types::value_representation::option_parameter(
            &self.checked.type_store,
            &self.checked.types.known_std_types,
            output,
        )
        .expect("checked payload conversion Option ABI");
        let result = match self.neutral_payload_types([input, target], span) {
            Ok(false) => None,
            Err(signal) => return signal,
            Ok(true) => {
                let mut budget = self.storage_limits.value_budget();
                match super::host_value::interp_to_bounded_host_value(value, &mut budget) {
                    Ok(value) => {
                        super::host_value::decode_checked_host_value(value, target, self.checked)
                            .ok()
                    }
                    Err(error) if error.code == HostErrorCode::SchemaMismatch => None,
                    Err(error) => return ControlSignal::runtime_fault(error.message, span),
                }
            }
        };
        if let Some(signal) = self.payload_conversion_control(span) {
            return signal;
        }
        if matches!(self.checked.type_store.get(output), Some(Type::Option(_))) {
            return ControlSignal::Value(match result {
                Some(value) => InterpValue::OptionSome(value.into()),
                None => InterpValue::OptionNone,
            });
        }
        let (name, fields) = match result {
            Some(value) => ("Some", vec![value]),
            None => ("None", Vec::new()),
        };
        match crate::value::enum_value::construct(output, name, fields, self.checked) {
            Ok(value) => ControlSignal::Value(value),
            Err(reason) => ControlSignal::missing_checked_fact(reason, span),
        }
    }

    fn payload_conversion_control(&self, span: Span) -> Option<ControlSignal> {
        self.cancellation_signal(span).or_else(|| {
            self.host_context
                .budget
                .check_time()
                .err()
                .map(|error| ControlSignal::runtime_fault(error.message, span))
        })
    }

    /// Traverse checked declaration templates and their actual arguments
    /// separately. Recursive generic ADTs stay finite, without enumerating
    /// Growing<T>, Growing<List<T>>, ... or requiring unused instances.
    fn neutral_payload_types(&self, roots: [TypeId; 2], span: Span) -> Result<bool, ControlSignal> {
        let store = &self.checked.type_store;
        let known = &self.checked.types.known_std_types;
        let mut pending = roots.into_iter().map(|ty| (ty, false)).collect::<Vec<_>>();
        let mut visited = std::collections::HashSet::new();
        while let Some((ty, template)) = pending.pop() {
            if !visited.insert((ty, template)) {
                continue;
            }
            if let Some(signal) = self.payload_conversion_control(span) {
                return Err(signal);
            }
            if visited.len() > self.storage_limits.max_nodes {
                return Err(ControlSignal::runtime_fault(
                    "payload type graph exceeds the current node budget",
                    span,
                ));
            }
            let data = store.get(ty).ok_or_else(|| {
                ControlSignal::missing_checked_fact("payload conversion type is absent", span)
            })?;
            if template && matches!(data, Type::Named(_)) {
                continue;
            }
            if !super::data_capture::neutral_identity(self.checked, ty) {
                return Ok(false);
            }
            if known.json_value == Some(ty) {
                continue;
            }
            match data {
                Type::Primitive(_) | Type::IntegerLiteral { .. } => {}
                Type::Array(inner)
                | Type::List(inner)
                | Type::Slice(inner)
                | Type::Set(inner)
                | Type::Option(inner)
                | Type::Refined { base: inner, .. } => pending.push((*inner, template)),
                Type::Map { key, value } => pending.extend([(*key, template), (*value, template)]),
                Type::Result { ok, err } => pending.extend([(*ok, template), (*err, template)]),
                Type::Tuple(fields) => pending.extend(fields.iter().map(|ty| (*ty, template))),
                Type::Record(record) => {
                    pending.extend(record.fields.iter().map(|field| (field.ty, template)))
                }
                Type::Applied { constructor, args } => {
                    pending.extend(args.iter().map(|ty| (*ty, template)));
                    pending.push((TypeId(constructor.0), true));
                }
                Type::Nominal(owner) => {
                    if !template && !owner.params.is_empty() {
                        return Ok(false);
                    }
                    let Some(representation) = owner.representation else {
                        return Ok(false);
                    };
                    pending.push((representation, template));
                }
                Type::Enum(_) => {
                    let layout = self.checked.types.enum_layouts.get(&ty).ok_or_else(|| {
                        ControlSignal::missing_checked_fact(
                            "payload enum has no checked layout",
                            span,
                        )
                    })?;
                    if !template && !layout.type_params.is_empty() {
                        return Ok(false);
                    }
                    pending.extend(
                        layout
                            .variants
                            .iter()
                            .flat_map(|variant| variant.fields.iter().map(|ty| (*ty, template))),
                    );
                }
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
}
