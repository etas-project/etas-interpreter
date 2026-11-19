//! Neutral value representation boundary; codecs and application policy are Etas.
use super::*;
use crate::value::{HostJsonSupportValue, NumericValue};
use etas_host::{HostError, HostErrorCode};
use etas_types::{Type, TypeId};

impl EvalContext<'_> {
    pub(super) fn execute_data_capture(
        &self,
        item: HirItemId,
        args: &[InterpValue],
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> ControlSignal {
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            unreachable!()
        };
        let [value] = args else {
            return ControlSignal::missing_checked_fact(
                "Data.capture requires one checked argument",
                span,
            );
        };
        let input = match self.materialized_type(signature.params[0], bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let result_type = match self.materialized_type(signature.output, bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let (data, err) = etas_types::value_representation::result_parameters(
            &self.checked.type_store,
            &self.checked.types.known_std_types,
            result_type,
        )
        .expect("checked Data.capture result ABI");
        let mut capture = Capture {
            ctx: self,
            data,
            span,
            budget: self.storage_limits.value_budget(),
        };
        let result = capture.check_execution().and_then(|()| {
            capture.check_identity(input)?;
            capture.value(value, 0)
        });
        let (name, value) = match result {
            Ok(value) => ("Ok", value),
            Err(CaptureFailure::Signal(signal)) => return *signal,
            Err(CaptureFailure::Value(error)) => {
                let value = HostValue::Record(vec![
                    (
                        "code".into(),
                        HostValue::String(
                            match error.code {
                                HostErrorCode::BudgetExceeded => "budget_exceeded",
                                _ => "schema_mismatch",
                            }
                            .into(),
                        ),
                    ),
                    ("message".into(), HostValue::String(error.message)),
                ]);
                match super::host_value::decode_checked_host_value(value, err, self.checked) {
                    Ok(value) => ("Err", value),
                    Err(error) => return ControlSignal::missing_checked_fact(error, span),
                }
            }
        };
        self.checked_result_value(result_type, name, value, span)
    }
}

enum CaptureFailure {
    Value(HostError),
    Signal(Box<ControlSignal>),
}
impl From<HostError> for CaptureFailure {
    fn from(error: HostError) -> Self {
        Self::Value(error)
    }
}
type CaptureResult<T> = Result<T, CaptureFailure>;

struct Capture<'a, 'program> {
    ctx: &'a EvalContext<'program>,
    data: TypeId,
    span: Span,
    budget: etas_host::ValueBudget<'a>,
}
impl Capture<'_, '_> {
    fn check_execution(&self) -> CaptureResult<()> {
        if let Some(signal) = self.ctx.cancellation_signal(self.span) {
            return Err(CaptureFailure::Signal(Box::new(signal)));
        }
        self.ctx.host_context.budget.check_time().map_err(|error| {
            CaptureFailure::Signal(Box::new(ControlSignal::runtime_fault(
                error.message,
                self.span,
            )))
        })
    }

    fn check_identity(&self, ty: TypeId) -> CaptureResult<()> {
        neutral_identity(self.ctx.checked, ty)
            .then_some(())
            .ok_or_else(|| rejected("execution, protected, or unresolved value cannot be captured"))
    }

    fn node(&mut self, depth: usize) -> CaptureResult<()> {
        self.check_execution()?;
        self.budget.add(depth, std::mem::size_of::<HostValue>())?;
        Ok(())
    }

    fn data(&self, name: &str, fields: Vec<InterpValue>) -> InterpValue {
        InterpValue::Nominal {
            ty: self.data,
            value: InterpValue::Variant {
                name: name.into(),
                fields: fields.into(),
            }
            .into(),
        }
    }

    fn value(&mut self, value: &InterpValue, depth: usize) -> CaptureResult<InterpValue> {
        if let InterpValue::Nominal { ty, value: inner } = value {
            self.check_identity(*ty)?;
            if *ty == self.data {
                self.neutral(inner, depth)?;
                return Ok(value.clone());
            }
            return self.value(inner, depth);
        }
        self.node(depth)?;
        let (name, fields) = match value {
            InterpValue::Unit => ("Unit", vec![]),
            InterpValue::Bool(_) => ("Bool", vec![value.clone()]),
            InterpValue::Number(number) => {
                if let Some(value) = number.as_i128() {
                    ("Int", vec![InterpValue::Number(NumericValue::I128(value))])
                } else if let Some(value) = number.as_u128() {
                    ("UInt", vec![InterpValue::Number(NumericValue::U128(value))])
                } else if let Some(value) = number.as_f64().filter(|v| v.is_finite()) {
                    (
                        "Float",
                        vec![InterpValue::Number(NumericValue::F64(value.to_bits()))],
                    )
                } else {
                    return Err(rejected("floating-point value is not finite"));
                }
            }
            InterpValue::String(text) => {
                self.budget.add(depth, text.len())?;
                ("String", vec![value.clone()])
            }
            InterpValue::Bytes(bytes) => {
                self.budget.add(depth, bytes.len())?;
                ("Bytes", vec![value.clone()])
            }
            InterpValue::Json(json) => {
                self.json(json, depth + 1)?;
                ("Json", vec![value.clone()])
            }
            InterpValue::Tuple(values) => ("List", vec![self.list(values.iter(), depth)?]),
            InterpValue::Array(values) | InterpValue::Stack(values) => {
                ("List", vec![self.list(values.borrow().iter(), depth)?])
            }
            InterpValue::List(values) => ("List", vec![self.list(values.iter(), depth)?]),
            InterpValue::Deque(values) | InterpValue::Queue(values) => {
                ("List", vec![self.list(values.borrow().iter(), depth)?])
            }
            InterpValue::Map(values) => (
                "Map",
                vec![self.map(values.borrow().iter().map(|(k, v)| (k, v)), depth)?],
            ),
            InterpValue::OrderedMap(values) if values.comparator().is_none() => (
                "Map",
                vec![self.map(values.iter().map(|(k, v)| (k, v)), depth)?],
            ),
            InterpValue::PriorityQueue(values) if values.comparator().is_none() => (
                "Map",
                vec![self.map(values.iter().map(|(k, v)| (k, v)), depth)?],
            ),
            InterpValue::Record(values) => {
                let fields = values
                    .borrow()
                    .iter()
                    .map(|(name, value)| {
                        self.budget.add(depth, name.len())?;
                        Ok(InterpValue::Tuple(
                            vec![
                                InterpValue::String(name.clone().into()),
                                self.value(value, depth + 1)?,
                            ]
                            .into(),
                        ))
                    })
                    .collect::<CaptureResult<Vec<_>>>()?;
                ("Record", vec![InterpValue::Array(ArrayValue::new(fields))])
            }
            InterpValue::Variant { name, fields } => {
                self.budget.add(depth, name.len())?;
                (
                    "Variant",
                    vec![
                        InterpValue::String(name.clone()),
                        self.list(fields.iter(), depth)?,
                    ],
                )
            }
            InterpValue::OptionNone => {
                self.budget.add(depth, 4)?;
                (
                    "Variant",
                    vec![
                        InterpValue::String("None".into()),
                        InterpValue::Array(ArrayValue::new(vec![])),
                    ],
                )
            }
            InterpValue::OptionSome(value) => {
                self.budget.add(depth, 4)?;
                (
                    "Variant",
                    vec![
                        InterpValue::String("Some".into()),
                        self.list([value.as_ref()], depth)?,
                    ],
                )
            }
            _ => {
                return Err(rejected(format!(
                    "{} cannot be captured as neutral data",
                    value.kind_name()
                )));
            }
        };
        Ok(self.data(name, fields))
    }

    fn list<'a>(
        &mut self,
        values: impl IntoIterator<Item = &'a InterpValue>,
        depth: usize,
    ) -> CaptureResult<InterpValue> {
        let values = values
            .into_iter()
            .map(|value| self.value(value, depth + 1))
            .collect::<CaptureResult<Vec<_>>>()?;
        Ok(InterpValue::Array(ArrayValue::new(values)))
    }
    fn map<'a>(
        &mut self,
        values: impl IntoIterator<Item = (&'a InterpValue, &'a InterpValue)>,
        depth: usize,
    ) -> CaptureResult<InterpValue> {
        let values = values
            .into_iter()
            .map(|(key, value)| {
                Ok(InterpValue::Tuple(
                    vec![self.value(key, depth + 1)?, self.value(value, depth + 1)?].into(),
                ))
            })
            .collect::<CaptureResult<Vec<_>>>()?;
        Ok(InterpValue::Array(ArrayValue::new(values)))
    }

    /// Data wrappers do not consume neutral nodes. Enforce exactly the same
    /// payload accounting as ordinary values before returning the shared value.
    fn neutral(&mut self, value: &InterpValue, depth: usize) -> CaptureResult<()> {
        let InterpValue::Variant { name, fields } = value else {
            return Err(rejected("invalid checked Data representation"));
        };
        self.node(depth)?;
        match (name.to_string().as_str(), fields.as_slice()) {
            ("Unit", []) | ("Bool" | "Int" | "UInt", [_]) => {}
            ("Float", [InterpValue::Number(value)])
                if value.as_f64().is_some_and(f64::is_finite) => {}
            ("String", [InterpValue::String(value)]) => self.budget.add(depth, value.len())?,
            ("Bytes", [InterpValue::Bytes(value)]) => self.budget.add(depth, value.len())?,
            ("List", [InterpValue::Array(values)]) => {
                for value in values.borrow() {
                    self.neutral_child(value, depth + 1)?;
                }
            }
            ("Map" | "Record", [InterpValue::Array(entries)]) => {
                let mut names = std::collections::HashSet::new();
                for entry in entries.borrow() {
                    let InterpValue::Tuple(pair) = entry else {
                        return Err(rejected("invalid checked Data entry"));
                    };
                    let [key, value] = pair.as_slice() else {
                        return Err(rejected("invalid checked Data entry"));
                    };
                    if name.to_string() == "Record" {
                        let InterpValue::String(key) = key else {
                            return Err(rejected("invalid checked Data record key"));
                        };
                        self.budget.add(depth, key.len())?;
                        if !names.insert(key.clone()) {
                            return Err(rejected("record contains duplicate field name"));
                        }
                    } else {
                        self.neutral_child(key, depth + 1)?;
                    }
                    self.neutral_child(value, depth + 1)?;
                }
            }
            ("Variant", [InterpValue::String(name), InterpValue::Array(values)]) => {
                self.budget.add(depth, name.len())?;
                for value in values.borrow() {
                    self.neutral_child(value, depth + 1)?;
                }
            }
            ("Json", [InterpValue::Json(json)]) => self.json(json, depth + 1)?,
            _ => {
                return Err(rejected(
                    "invalid or non-finite checked Data representation",
                ));
            }
        }
        Ok(())
    }
    fn neutral_child(&mut self, value: &InterpValue, depth: usize) -> CaptureResult<()> {
        let InterpValue::Nominal { ty, value } = value else {
            return Err(rejected("invalid checked Data child"));
        };
        if *ty != self.data {
            return Err(rejected("invalid checked Data child identity"));
        }
        self.neutral(value, depth)
    }
    fn json(&mut self, value: &HostJsonSupportValue, depth: usize) -> CaptureResult<()> {
        self.check_execution()?;
        self.budget
            .add(depth, std::mem::size_of::<etas_host::HostJsonValue>())?;
        match value {
            HostJsonSupportValue::NumberBits(bits) if !f64::from_bits(*bits).is_finite() => {
                return Err(rejected("JSON number is not finite"));
            }
            HostJsonSupportValue::String(value) => self.budget.add(depth, value.len())?,
            HostJsonSupportValue::Array(values) => {
                for value in values.iter() {
                    self.json(value, depth + 1)?;
                }
            }
            HostJsonSupportValue::Object(values) => {
                for (name, value) in values.iter() {
                    self.budget.add(depth, name.len())?;
                    self.json(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Shared identity gate for neutral capture and type-directed payload ingress.
/// Ordinary source declarations are inspected through their representation;
/// private fields by themselves do not imply an execution capability.
pub(super) fn neutral_identity(checked: &etas_frontend::CheckedProject, ty: TypeId) -> bool {
    use etas_std::RuntimeTypeRole as R;
    let known = &checked.types.known_std_types;
    let store = &checked.type_store;
    if etas_types::source_execution::is_scoped_handle(store, known, ty) {
        return false;
    }
    let constructor = match store.get(ty) {
        Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
        _ => ty,
    };
    if let Some(role) = known.runtime_role(store, ty) {
        return matches!(
            role,
            R::JsonValue
                | R::Deque
                | R::Queue
                | R::Stack
                | R::PriorityQueue
                | R::OrderedMap
                | R::OrderedSet
                | R::Money
                | R::SessionId
                | R::ContextPolicy
                | R::RetentionPolicy
                | R::Limit
        );
    }
    if known.message == Some(constructor) || known.opaque_values.contains(&constructor) {
        return false;
    }
    match store.get(ty) {
        Some(
            Type::Primitive(_)
            | Type::IntegerLiteral { .. }
            | Type::Array(_)
            | Type::List(_)
            | Type::Slice(_)
            | Type::Tuple(_)
            | Type::Map { .. }
            | Type::Set(_)
            | Type::Record(_)
            | Type::Enum(_)
            | Type::Nominal(_)
            | Type::Option(_)
            | Type::Result { .. },
        ) => true,
        Some(Type::Applied { constructor, .. }) => neutral_identity(checked, TypeId(constructor.0)),
        Some(Type::Refined { base, .. }) => neutral_identity(checked, *base),
        _ => false,
    }
}
fn rejected(message: impl Into<String>) -> CaptureFailure {
    HostError::new(HostErrorCode::SchemaMismatch, message).into()
}
