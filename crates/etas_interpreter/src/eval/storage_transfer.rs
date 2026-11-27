//! Typed value ABI for storage. Binary encoding and collection algorithms live
//! in Etas; this machine moves values through checked layouts and Etas calls.
use super::*;
use crate::value::{NumericValue as N, RangeBounds, RangeValue};
use etas_types::storage_layout::{
    StorageCall, StorageCollection as C, StorageKind as K, StorageLayout, StorageNodeId as Id,
};
use etas_types::{PrimitiveType as P, TrustWrapper, TypeId};
use std::sync::Arc;

type TransferResult<T> = Result<T, String>;

#[derive(Clone, Copy, Debug)]
enum Direction {
    Capture,
    Restore,
}

#[derive(Clone, Debug)]
enum Payload {
    Direct,
    Array,
    Pairs,
    Fields(Vec<String>),
}

#[derive(Clone, Debug)]
enum Build {
    Encoded {
        name: &'static str,
        prefix: Vec<InterpValue>,
        payload: Payload,
    },
    Tuple,
    Array,
    List,
    Slice,
    Set,
    Map,
    Record(Vec<String>),
    Nominal(TypeId),
    Variant {
        ty: Option<TypeId>,
        name: String,
        positions: Vec<usize>,
    },
    Option(bool),
    Trust {
        wrapper: TrustWrapper,
        nominal: Option<TypeId>,
    },
    Range(RangeBounds),
    Collection(Id),
}

#[derive(Clone, Debug)]
enum Work {
    Visit(Id, InterpValue, usize),
    Build(Build, usize),
    RestoreCollection(Id),
}

#[derive(Clone, Debug)]
enum Awaiting {
    View(Id, usize),
    Built { ty: TypeId, fallible: bool },
}

#[derive(Clone, Debug)]
pub struct StorageTransfer {
    layout: Arc<StorageLayout>,
    direction: Direction,
    data: TypeId,
    result: TypeId,
    error: TypeId,
    work: Vec<Work>,
    values: Vec<InterpValue>,
    awaiting: Option<Awaiting>,
    nodes: usize,
    bytes: usize,
    span: Span,
}

fn mismatch() -> String {
    "stored value differs from its checked storage schema".into()
}
fn u8_value(value: u8) -> InterpValue {
    InterpValue::Number(N::U8(value))
}
fn text(value: impl Into<String>) -> InterpValue {
    InterpValue::String(value.into().into())
}
fn array(values: Vec<InterpValue>) -> InterpValue {
    InterpValue::Array(values.into())
}

impl EvalContext<'_> {
    pub(super) fn execute_storage_transfer(
        &mut self,
        item: HirItemId,
        role: etas_std::StdExecutionRole,
        args: &[InterpValue],
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> ControlSignal {
        use etas_std::StdExecutionRole as R;
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            unreachable!()
        };
        let ty = match self.materialized_type(signature.generic_params[0].subject, bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let key = (Arc::as_ptr(self.checked) as usize, ty);
        let layout = match self.runtime.storage_layouts.get(&key) {
            Some((_, layout)) => layout.clone(),
            None => {
                let description = match self
                    .checked
                    .describe_storage_layout(ty, etas_code::descriptor::DescriptorLimits::default())
                {
                    Ok(description) => description,
                    Err(error) => {
                        return ControlSignal::missing_checked_fact(error.to_string(), span);
                    }
                };
                let layout = Arc::new(description.layout);
                // Hold the program with the address key: a later revision can
                // never reuse an address whose TypeIds belong to this cache.
                self.runtime
                    .storage_layouts
                    .insert(key, (self.checked.clone(), layout.clone()));
                layout
            }
        };
        if role == R::StorageSchema {
            return ControlSignal::Value(text(layout.schema.to_string()));
        }
        let result = match self.materialized_type(signature.output, bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let (ok, error) = etas_types::value_representation::result_parameters(
            &self.checked.type_store,
            &self.checked.types.known_std_types,
            result,
        )
        .expect("checked storage mechanism result ABI");
        let direction = if role == R::CaptureStorage {
            Direction::Capture
        } else {
            Direction::Restore
        };
        let data = if role == R::CaptureStorage {
            ok
        } else {
            signature.params[0]
        };
        let state = StorageTransfer {
            work: vec![Work::Visit(layout.root, args[0].clone(), 0)],
            layout,
            direction,
            data,
            result,
            error,
            values: vec![],
            awaiting: None,
            nodes: 0,
            bytes: 0,
            span,
        };
        self.continue_storage_transfer(state, InterpValue::Unit)
    }

    pub(crate) fn continue_storage_transfer(
        &mut self,
        mut state: StorageTransfer,
        value: InterpValue,
    ) -> ControlSignal {
        if let Some(awaiting) = state.awaiting.take() {
            match awaiting {
                Awaiting::View(view, depth) => state.work.push(Work::Visit(view, value, depth)),
                Awaiting::Built { ty, fallible } => {
                    let value = if fallible {
                        let output = match &value {
                            InterpValue::Nominal { ty: output, .. }
                                if etas_types::value_representation::option_parameter(
                                    &self.checked.type_store,
                                    &self.checked.types.known_std_types,
                                    *output,
                                ) == Some(ty) =>
                            {
                                *output
                            }
                            _ => {
                                return ControlSignal::missing_checked_fact(
                                    "storage adapter Option result was not prepared",
                                    state.span,
                                );
                            }
                        };
                        match self.plan.prepared_program().enum_payload(&value, output) {
                            Ok(("Some", [value])) => value.clone(),
                            Ok(("None", [])) => {
                                return self.storage_transfer_error(
                                    &state,
                                    "schema_mismatch",
                                    mismatch(),
                                );
                            }
                            _ => {
                                return ControlSignal::missing_checked_fact(
                                    "storage adapter has a different checked Option ABI",
                                    state.span,
                                );
                            }
                        }
                    } else {
                        value
                    };
                    state.values.push(value);
                }
            }
        }
        while let Some(work) = state.work.pop() {
            if let Err(fault) = self.consume_execution_step(state.span) {
                return ControlSignal::Fault(Box::new(fault));
            }
            let step = match work {
                Work::Visit(id, value, depth) => {
                    state.nodes += 1;
                    state.bytes = state.bytes.saturating_add(match &value {
                        InterpValue::String(value) => value.len(),
                        InterpValue::Bytes(value) => value.len(),
                        _ => 0,
                    });
                    if depth > self.storage_limits.max_depth
                        || state.nodes > self.storage_limits.max_nodes
                        || state.bytes > self.storage_limits.max_value_bytes
                    {
                        return self.storage_transfer_error(
                            &state,
                            "budget_exceeded",
                            "storage value exceeds its runtime size, node or depth budget".into(),
                        );
                    }
                    match state.direction {
                        Direction::Capture => state.capture(self, id, value, depth),
                        Direction::Restore => state.restore(self, id, value, depth),
                    }
                }
                Work::Build(build, count) => state.build(self, build, count).map(|()| None),
                Work::RestoreCollection(id) => {
                    let K::Collection {
                        restore,
                        fallible_restore,
                        ..
                    } = &state.layout.nodes[id.0].kind
                    else {
                        unreachable!()
                    };
                    let value = state
                        .values
                        .pop()
                        .expect("restored logical collection view");
                    Ok(Some((
                        restore.clone(),
                        value,
                        Awaiting::Built {
                            ty: state.layout.nodes[id.0].ty,
                            fallible: *fallible_restore,
                        },
                    )))
                }
            };
            match step {
                Err(error) => return self.storage_transfer_error(&state, "schema_mismatch", error),
                Ok(None) => {}
                Ok(Some((call, value, awaiting))) => {
                    let target = match self.storage_adapter_target(&call) {
                        Ok(target) => target,
                        Err(error) => {
                            return ControlSignal::missing_checked_fact(error, state.span);
                        }
                    };
                    state.awaiting = Some(awaiting);
                    return ControlSignal::pending_call(PendingCall {
                        purity: crate::control::CallPurity::Pure,
                        site: None,
                        target,
                        args: vec![value],
                        span: state.span,
                        continuation: Continuation::StorageTransfer(Box::new(state)),
                    });
                }
            }
        }
        self.checked_result_value(
            state.result,
            "Ok",
            state.values.pop().expect("completed storage value"),
            state.span,
        )
    }

    fn storage_transfer_error(
        &self,
        state: &StorageTransfer,
        code: &str,
        message: String,
    ) -> ControlSignal {
        let error = HostValue::Record(vec![
            ("code".into(), HostValue::String(code.into())),
            ("message".into(), HostValue::String(message)),
            (
                "operation".into(),
                HostValue::Variant {
                    name: "None".into(),
                    fields: vec![],
                },
            ),
        ]);
        match super::host_value::decode_checked_host_value(error, state.error, self.checked) {
            Ok(error) => self.checked_result_value(state.result, "Err", error, state.span),
            Err(error) => ControlSignal::missing_checked_fact(error, state.span),
        }
    }

    pub(in crate::eval) fn storage_adapter_target(
        &self,
        call: &StorageCall,
    ) -> TransferResult<CallTarget> {
        let target = match &call.target {
            etas_types::method::MethodTarget::Source { callable, .. } => {
                CallTarget::FlowItem(*callable)
            }
            etas_types::method::MethodTarget::External { identity } => {
                if self.checked.foreign_method(identity).is_some() {
                    self.foreign
                        .as_ref()
                        .ok_or("storage adapter has no live caller owner")?
                        .method(self.checked, identity)?
                } else {
                    self.plan
                        .standard_library
                        .as_ref()
                        .ok_or("storage adapter has no executable library")?
                        .method(identity)?
                }
            }
            _ => return Err("storage adapter requires a checked Etas body".into()),
        };
        Ok(CallTarget::Specialized {
            target: target.into(),
            type_bindings: vec![],
            type_parameter_bindings: call.owner_bindings.clone(),
            effect_row_bindings: vec![],
        })
    }
}

type Invoke = Option<(StorageCall, InterpValue, Awaiting)>;

impl StorageTransfer {
    fn encoded(
        &self,
        ctx: &EvalContext<'_>,
        name: &str,
        fields: Vec<InterpValue>,
    ) -> TransferResult<InterpValue> {
        ctx.plan
            .prepared_program()
            .construct_enum(self.data, name, fields)
    }
    fn read(
        &self,
        ctx: &EvalContext<'_>,
        value: &InterpValue,
    ) -> TransferResult<(String, Vec<InterpValue>)> {
        let (name, values) = ctx.plan.prepared_program().enum_payload(value, self.data)?;
        Ok((name.into(), values.to_vec()))
    }
    fn identity(&self, id: Id) -> String {
        self.layout.nodes[id.0]
            .identity
            .as_ref()
            .expect("checked named storage identity")
            .fingerprint()
            .to_string()
    }
    fn children(&mut self, build: Build, children: Vec<(Id, InterpValue)>, depth: usize) {
        self.work.push(Work::Build(build, children.len()));
        self.work.extend(
            children
                .into_iter()
                .rev()
                .map(|(id, value)| Work::Visit(id, value, depth + 1)),
        );
    }
    fn storage(
        &mut self,
        name: &'static str,
        prefix: Vec<InterpValue>,
        payload: Payload,
        children: Vec<(Id, InterpValue)>,
        depth: usize,
    ) {
        self.children(
            Build::Encoded {
                name,
                prefix,
                payload,
            },
            children,
            depth,
        );
    }
    fn capture(
        &mut self,
        ctx: &EvalContext<'_>,
        id: Id,
        value: InterpValue,
        depth: usize,
    ) -> TransferResult<Invoke> {
        let node = self.layout.nodes[id.0].clone();
        match node.kind {
            K::Primitive(kind) => {
                let (name, fields) = capture_primitive(kind, value)?;
                self.values.push(self.encoded(ctx, name, fields)?);
            }
            K::Tuple(types) => {
                let InterpValue::Tuple(values) = value else {
                    return Err(mismatch());
                };
                if types.len() != values.len() {
                    return Err(mismatch());
                }
                self.storage(
                    "Sequence",
                    vec![u8_value(64)],
                    Payload::Array,
                    types.into_iter().zip(values.iter().cloned()).collect(),
                    depth,
                );
            }
            K::Array(inner) | K::List(inner) | K::Slice(inner) | K::Set(inner) => {
                let (tag, values) = match value {
                    InterpValue::Array(values) => (65, values.borrow().to_vec()),
                    InterpValue::List(values) => (66, values.iter().cloned().collect()),
                    InterpValue::Slice(values) => (67, values.borrow().to_vec()),
                    InterpValue::Set(values) => (71, values.borrow().to_vec()),
                    _ => return Err(mismatch()),
                };
                self.storage(
                    "Sequence",
                    vec![u8_value(tag)],
                    Payload::Array,
                    values.into_iter().map(|value| (inner, value)).collect(),
                    depth,
                );
            }
            K::Map(key, item) => {
                let InterpValue::Map(values) = value else {
                    return Err(mismatch());
                };
                self.storage(
                    "Map",
                    vec![],
                    Payload::Pairs,
                    values
                        .borrow()
                        .iter()
                        .flat_map(|(k, v)| [(key, k.clone()), (item, v.clone())])
                        .collect(),
                    depth,
                );
            }
            K::Record(fields) => {
                let InterpValue::Record(values) = value else {
                    return Err(mismatch());
                };
                let children = fields
                    .iter()
                    .map(|field| {
                        Ok((
                            field.node,
                            values.get_ref(&field.name).ok_or_else(mismatch)?.clone(),
                        ))
                    })
                    .collect::<TransferResult<Vec<_>>>()?;
                self.storage(
                    "Record",
                    vec![],
                    Payload::Fields(fields.into_iter().map(|field| field.name).collect()),
                    children,
                    depth,
                );
            }
            K::Nominal { representation } => {
                let InterpValue::Nominal { ty, value } = value else {
                    return Err(mismatch());
                };
                if ty != node.ty {
                    return Err(mismatch());
                }
                self.storage(
                    "Nominal",
                    vec![text(self.identity(id))],
                    Payload::Direct,
                    vec![(representation, value.as_ref().clone())],
                    depth,
                );
            }
            K::Enum(variants) => {
                let (name, values) = ctx.plan.prepared_program().enum_payload(&value, node.ty)?;
                let variant = variants
                    .iter()
                    .find(|variant| variant.name == name)
                    .ok_or_else(mismatch)?;
                let children = variant
                    .fields
                    .iter()
                    .map(|field| (field.node, values[field.position].clone()))
                    .collect();
                self.storage(
                    "Variant",
                    vec![text(self.identity(id)), text(name)],
                    Payload::Array,
                    children,
                    depth,
                );
            }
            K::Collection { capture, view, .. } => {
                self.work.push(Work::Build(Build::Collection(id), 1));
                return Ok(Some((capture, value, Awaiting::View(view, depth + 1))));
            }
            K::Option(inner) => {
                let (name, children) = match value {
                    InterpValue::OptionNone => ("None", vec![]),
                    InterpValue::OptionSome(value) => {
                        ("Some", vec![(inner, value.as_ref().clone())])
                    }
                    _ => return Err(mismatch()),
                };
                self.storage(
                    "Variant",
                    vec![text("etas.core.Option"), text(name)],
                    Payload::Array,
                    children,
                    depth,
                );
            }
            K::Result(ok, err) => {
                let InterpValue::Variant { name, fields } = value else {
                    return Err(mismatch());
                };
                let inner = match name.as_ref() {
                    "Ok" => ok,
                    "Err" => err,
                    _ => return Err(mismatch()),
                };
                self.storage(
                    "Variant",
                    vec![text("etas.core.Result"), text(name.to_string())],
                    Payload::Array,
                    vec![(inner, fields[0].clone())],
                    depth,
                );
            }
            K::Trust { wrapper, inner } => {
                let value = match value {
                    InterpValue::Trust {
                        wrapper: actual,
                        value,
                    } if actual == wrapper => value.as_ref().clone(),
                    InterpValue::Nominal { ty, value }
                        if ty == node.ty && node.identity.is_some() =>
                    {
                        let InterpValue::Record(fields) = value.as_ref() else {
                            return Err(mismatch());
                        };
                        fields.get_ref("value").ok_or_else(mismatch)?.clone()
                    }
                    _ => return Err(mismatch()),
                };
                self.storage(
                    "Trust",
                    vec![
                        u8_value(trust_label(wrapper)?),
                        InterpValue::Bytes(vec![].into()),
                    ],
                    Payload::Direct,
                    vec![(inner, value)],
                    depth,
                );
            }
            K::Range(inner) => {
                let InterpValue::Range(range) = value else {
                    return Err(mismatch());
                };
                self.storage(
                    "Variant",
                    vec![text("etas.core.Range"), text(range_name(range.bounds))],
                    Payload::Array,
                    vec![(inner, *range.start), (inner, *range.end)],
                    depth,
                );
            }
        }
        Ok(None)
    }

    fn restore(
        &mut self,
        ctx: &EvalContext<'_>,
        id: Id,
        value: InterpValue,
        depth: usize,
    ) -> TransferResult<Invoke> {
        let node = self.layout.nodes[id.0].clone();
        let (name, fields) = self.read(ctx, &value)?;
        match node.kind {
            K::Primitive(kind) => self.values.push(restore_primitive(kind, &name, &fields)?),
            K::Tuple(types) => {
                let values = sequence(&name, &fields, 64)?;
                if values.len() != types.len() {
                    return Err(mismatch());
                }
                self.children(Build::Tuple, types.into_iter().zip(values).collect(), depth);
            }
            K::Array(inner) | K::List(inner) | K::Slice(inner) | K::Set(inner) => {
                let (tag, build) = match node.kind {
                    K::Array(_) => (65, Build::Array),
                    K::List(_) => (66, Build::List),
                    K::Slice(_) => (67, Build::Slice),
                    K::Set(_) => (71, Build::Set),
                    _ => unreachable!(),
                };
                self.children(
                    build,
                    sequence(&name, &fields, tag)?
                        .into_iter()
                        .map(|value| (inner, value))
                        .collect(),
                    depth,
                );
            }
            K::Map(key, item) => {
                if name != "Map" {
                    return Err(mismatch());
                }
                let values = items(&fields[0])?;
                let children = values
                    .into_iter()
                    .map(|pair| {
                        let InterpValue::Tuple(pair) = pair else {
                            return Err(mismatch());
                        };
                        let [k, v] = &*pair else {
                            return Err(mismatch());
                        };
                        Ok([(key, k.clone()), (item, v.clone())])
                    })
                    .collect::<TransferResult<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                self.children(Build::Map, children, depth);
            }
            K::Record(layout) => {
                if name != "Record" {
                    return Err(mismatch());
                }
                let values = items(&fields[0])?;
                if values.len() != layout.len() {
                    return Err(mismatch());
                }
                let children = layout
                    .iter()
                    .zip(values)
                    .map(|(field, pair)| {
                        let InterpValue::Tuple(pair) = pair else {
                            return Err(mismatch());
                        };
                        let [InterpValue::String(name), value] = &*pair else {
                            return Err(mismatch());
                        };
                        if name.as_ref() != field.name {
                            return Err(mismatch());
                        }
                        Ok((field.node, value.clone()))
                    })
                    .collect::<TransferResult<Vec<_>>>()?;
                self.children(
                    Build::Record(layout.into_iter().map(|field| field.name).collect()),
                    children,
                    depth,
                );
            }
            K::Nominal { representation } => {
                if name != "Nominal" || fields[0] != text(self.identity(id)) {
                    return Err(mismatch());
                }
                self.children(
                    Build::Nominal(node.ty),
                    vec![(representation, fields[1].clone())],
                    depth,
                );
            }
            K::Enum(variants) => {
                if name != "Variant" || fields[0] != text(self.identity(id)) {
                    return Err(mismatch());
                }
                let InterpValue::String(name) = &fields[1] else {
                    return Err(mismatch());
                };
                let variant = variants
                    .iter()
                    .find(|variant| variant.name == name.as_ref())
                    .ok_or_else(mismatch)?;
                let values = items(&fields[2])?;
                if values.len() != variant.fields.len() {
                    return Err(mismatch());
                }
                self.children(
                    Build::Variant {
                        ty: Some(node.ty),
                        name: name.to_string(),
                        positions: variant.fields.iter().map(|field| field.position).collect(),
                    },
                    variant
                        .fields
                        .iter()
                        .zip(values)
                        .map(|(field, value)| (field.node, value))
                        .collect(),
                    depth,
                );
            }
            K::Collection { kind, view, .. } => {
                if name != "Nominal" || fields[0] != text(self.identity(id)) {
                    return Err(mismatch());
                }
                let value = self.collection_view(ctx, kind, fields[1].clone(), false)?;
                self.work.push(Work::RestoreCollection(id));
                self.work.push(Work::Visit(view, value, depth + 1));
            }
            K::Option(inner) => {
                if name != "Variant" || fields[0] != text("etas.core.Option") {
                    return Err(mismatch());
                }
                let values = items(&fields[2])?;
                let some = match (&fields[1], values.len()) {
                    (InterpValue::String(name), 0) if name.as_ref() == "None" => false,
                    (InterpValue::String(name), 1) if name.as_ref() == "Some" => true,
                    _ => return Err(mismatch()),
                };
                self.children(
                    Build::Option(some),
                    values.into_iter().map(|value| (inner, value)).collect(),
                    depth,
                );
            }
            K::Result(ok, err) => {
                if name != "Variant" || fields[0] != text("etas.core.Result") {
                    return Err(mismatch());
                }
                let values = items(&fields[2])?;
                let (variant, inner) = match &fields[1] {
                    InterpValue::String(name) if name.as_ref() == "Ok" => ("Ok", ok),
                    InterpValue::String(name) if name.as_ref() == "Err" => ("Err", err),
                    _ => return Err(mismatch()),
                };
                if values.len() != 1 {
                    return Err(mismatch());
                }
                self.children(
                    Build::Variant {
                        ty: None,
                        name: variant.into(),
                        positions: vec![0],
                    },
                    vec![(inner, values[0].clone())],
                    depth,
                );
            }
            K::Trust { wrapper, inner } => {
                if name != "Trust"
                    || fields[0] != u8_value(trust_label(wrapper)?)
                    || !matches!(&fields[1],InterpValue::Bytes(proof) if proof.is_empty())
                {
                    return Err(mismatch());
                }
                self.children(
                    Build::Trust {
                        wrapper,
                        nominal: node.identity.as_ref().map(|_| node.ty),
                    },
                    vec![(inner, fields[2].clone())],
                    depth,
                );
            }
            K::Range(inner) => {
                if name != "Variant" || fields[0] != text("etas.core.Range") {
                    return Err(mismatch());
                }
                let InterpValue::String(name) = &fields[1] else {
                    return Err(mismatch());
                };
                let bounds = match name.as_ref() {
                    "ClosedClosed" => RangeBounds::ClosedClosed,
                    "ClosedOpen" => RangeBounds::ClosedOpen,
                    "OpenOpen" => RangeBounds::OpenOpen,
                    "OpenClosed" => RangeBounds::OpenClosed,
                    _ => return Err(mismatch()),
                };
                let values = items(&fields[2])?;
                if values.len() != 2 {
                    return Err(mismatch());
                }
                self.children(
                    Build::Range(bounds),
                    values.into_iter().map(|value| (inner, value)).collect(),
                    depth,
                );
            }
        }
        Ok(None)
    }

    fn build(&mut self, ctx: &EvalContext<'_>, build: Build, count: usize) -> TransferResult<()> {
        let mut values = self.values.split_off(self.values.len() - count);
        let value = match build {
            Build::Encoded {
                name,
                mut prefix,
                payload,
            } => {
                match payload {
                    Payload::Direct => prefix.extend(values),
                    Payload::Array => prefix.push(array(values)),
                    Payload::Pairs => prefix.push(array(
                        values
                            .chunks_exact(2)
                            .map(|pair| InterpValue::Tuple(pair.to_vec().into()))
                            .collect(),
                    )),
                    Payload::Fields(names) => prefix.push(array(
                        names
                            .into_iter()
                            .zip(values)
                            .map(|(name, value)| InterpValue::Tuple(vec![text(name), value].into()))
                            .collect(),
                    )),
                }
                self.encoded(ctx, name, prefix)?
            }
            Build::Tuple => InterpValue::Tuple(values.into()),
            Build::Array => array(values),
            Build::List => InterpValue::List(values.into()),
            Build::Slice => InterpValue::Slice(values.into()),
            Build::Set => InterpValue::Set(values.into()),
            Build::Map => InterpValue::Map(
                values
                    .chunks_exact(2)
                    .map(|pair| (pair[0].clone(), pair[1].clone()))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            Build::Record(names) => {
                InterpValue::Record(names.into_iter().zip(values).collect::<Vec<_>>().into())
            }
            Build::Nominal(ty) => InterpValue::Nominal {
                ty,
                value: values.pop().unwrap().into(),
            },
            Build::Variant {
                ty,
                name,
                positions,
            } => {
                let mut fields = vec![InterpValue::Unit; values.len()];
                for (position, value) in positions.into_iter().zip(values) {
                    fields[position] = value;
                }
                if let Some(ty) = ty {
                    ctx.plan
                        .prepared_program()
                        .construct_enum(ty, &name, fields)?
                } else {
                    InterpValue::Variant {
                        name: name.into(),
                        fields: fields.into(),
                    }
                }
            }
            Build::Option(some) => {
                if some {
                    InterpValue::OptionSome(values.pop().unwrap().into())
                } else {
                    InterpValue::OptionNone
                }
            }
            Build::Trust { wrapper, nominal } => {
                let value = values.pop().unwrap();
                match nominal {
                    Some(ty) => InterpValue::Nominal {
                        ty,
                        value: InterpValue::Record(vec![("value".into(), value)].into()).into(),
                    },
                    None => InterpValue::Trust {
                        wrapper,
                        value: value.into(),
                    },
                }
            }
            Build::Range(bounds) => {
                let end = values.pop().unwrap();
                let start = values.pop().unwrap();
                InterpValue::Range(RangeValue {
                    start: Box::new(start),
                    end: Box::new(end),
                    bounds,
                })
            }
            Build::Collection(id) => {
                let K::Collection { kind, .. } = self.layout.nodes[id.0].kind else {
                    unreachable!()
                };
                let value = self.collection_view(ctx, kind, values.pop().unwrap(), true)?;
                self.encoded(ctx, "Nominal", vec![text(self.identity(id)), value])?
            }
        };
        self.values.push(value);
        Ok(())
    }

    /// Reshape checked logical views into the format's map/set tags so the
    /// Etas encoder, rather than Rust, applies canonical key ordering.
    fn collection_view(
        &self,
        ctx: &EvalContext<'_>,
        kind: C,
        value: InterpValue,
        capture: bool,
    ) -> TransferResult<InterpValue> {
        if kind == C::BoundedDeque {
            return Ok(value);
        }
        let (name, fields) = self.read(ctx, &value)?;
        if kind == C::Map {
            if capture {
                let pairs = sequence(&name, &fields, 65)?
                    .into_iter()
                    .map(|pair| {
                        let (name, fields) = self.read(ctx, &pair)?;
                        let values = sequence(&name, &fields, 64)?;
                        if values.len() != 2 {
                            return Err(mismatch());
                        }
                        Ok(InterpValue::Tuple(values.into()))
                    })
                    .collect::<TransferResult<Vec<_>>>()?;
                return self.encoded(ctx, "Map", vec![array(pairs)]);
            }
            if name != "Map" {
                return Err(mismatch());
            }
            let pairs = items(&fields[0])?
                .into_iter()
                .map(|pair| {
                    let InterpValue::Tuple(values) = pair else {
                        return Err(mismatch());
                    };
                    self.encoded(ctx, "Sequence", vec![u8_value(64), array(values.to_vec())])
                })
                .collect::<TransferResult<Vec<_>>>()?;
            return self.encoded(ctx, "Sequence", vec![u8_value(65), array(pairs)]);
        }
        let tag = match kind {
            C::Set => 71,
            C::Slice => 67,
            C::Stack => 68,
            C::Queue => 69,
            C::Deque => 70,
            _ => unreachable!(),
        };
        let values = sequence(&name, &fields, if capture { 65 } else { tag })?;
        self.encoded(
            ctx,
            "Sequence",
            vec![u8_value(if capture { tag } else { 65 }), array(values)],
        )
    }
}

fn items(value: &InterpValue) -> TransferResult<Vec<InterpValue>> {
    match value {
        InterpValue::Array(values) => Ok(values.borrow().to_vec()),
        _ => Err(mismatch()),
    }
}
fn sequence(name: &str, fields: &[InterpValue], tag: u8) -> TransferResult<Vec<InterpValue>> {
    if name != "Sequence" || fields[0] != u8_value(tag) {
        return Err(mismatch());
    }
    items(&fields[1])
}
fn trust_label(wrapper: TrustWrapper) -> TransferResult<u8> {
    match wrapper {
        TrustWrapper::Public => Ok(0),
        TrustWrapper::Untrusted => Ok(1),
        _ => Err("protected storage provenance is not available".into()),
    }
}
fn range_name(bounds: RangeBounds) -> &'static str {
    match bounds {
        RangeBounds::ClosedClosed => "ClosedClosed",
        RangeBounds::ClosedOpen => "ClosedOpen",
        RangeBounds::OpenOpen => "OpenOpen",
        RangeBounds::OpenClosed => "OpenClosed",
    }
}
fn scalar(kind: P) -> Option<(u8, u8)> {
    Some(match kind {
        P::I8 => (16, 1),
        P::I16 => (17, 2),
        P::I32 => (18, 4),
        P::I64 => (19, 8),
        P::I128 => (20, 16),
        P::ISize => (21, 8),
        P::U8 => (24, 1),
        P::U16 => (25, 2),
        P::U32 => (26, 4),
        P::U64 => (27, 8),
        P::U128 => (28, 16),
        P::USize => (29, 8),
        P::F32 => (32, 4),
        P::F64 => (33, 8),
        P::Char => (34, 4),
        _ => return None,
    })
}
fn capture_primitive(
    kind: P,
    value: InterpValue,
) -> TransferResult<(&'static str, Vec<InterpValue>)> {
    let leaf = match (kind, &value) {
        (P::Unit, InterpValue::Unit) => Some(("Unit", vec![])),
        (P::Bool, InterpValue::Bool(_)) => Some(("Bool", vec![value.clone()])),
        (P::String, InterpValue::String(_)) => Some(("String", vec![value.clone()])),
        (P::Bytes, InterpValue::Bytes(_)) => Some(("Bytes", vec![value.clone()])),
        _ => None,
    };
    if let Some(leaf) = leaf {
        return Ok(leaf);
    }
    let (tag, width) = scalar(kind).ok_or_else(mismatch)?;
    let bits = match value {
        InterpValue::Number(number) if number.primitive() == kind => number.raw_bits(),
        InterpValue::String(value) if kind == P::Char => {
            let mut chars = value.chars();
            let value = chars.next().ok_or_else(mismatch)?;
            if chars.next().is_some() {
                return Err(mismatch());
            }
            value as u32 as u128
        }
        _ => return Err(mismatch()),
    };
    Ok((
        "Scalar",
        vec![
            u8_value(tag),
            u8_value(width),
            InterpValue::Number(N::U128(bits)),
        ],
    ))
}
fn restore_primitive(kind: P, name: &str, fields: &[InterpValue]) -> TransferResult<InterpValue> {
    match (kind, name, fields) {
        (P::Unit, "Unit", []) => return Ok(InterpValue::Unit),
        (P::Bool, "Bool", [value @ InterpValue::Bool(_)])
        | (P::String, "String", [value @ InterpValue::String(_)])
        | (P::Bytes, "Bytes", [value @ InterpValue::Bytes(_)]) => return Ok(value.clone()),
        _ => {}
    }
    let (tag, width) = scalar(kind).ok_or_else(mismatch)?;
    if name != "Scalar" {
        return Err(mismatch());
    }
    let [a, b, InterpValue::Number(N::U128(bits))] = fields else {
        return Err(mismatch());
    };
    if *a != u8_value(tag)
        || *b != u8_value(width)
        || (width < 16 && *bits >> (width as usize * 8) != 0)
    {
        return Err(mismatch());
    }
    let bits = *bits;
    let value = match kind {
        P::I8 => N::I8(bits as i8),
        P::I16 => N::I16(bits as i16),
        P::I32 => N::I32(bits as i32),
        P::I64 => N::I64(bits as i64),
        P::I128 => N::I128(bits as i128),
        P::ISize => N::ISize(bits as i64),
        P::U8 => N::U8(bits as u8),
        P::U16 => N::U16(bits as u16),
        P::U32 => N::U32(bits as u32),
        P::U64 => N::U64(bits as u64),
        P::U128 => N::U128(bits),
        P::USize => N::USize(bits as u64),
        P::F32 => N::F32(bits as u32),
        P::F64 => N::F64(bits as u64),
        P::Char => {
            return char::from_u32(bits as u32)
                .map(|value| text(value.to_string()))
                .ok_or_else(mismatch);
        }
        _ => return Err(mismatch()),
    };
    Ok(InterpValue::Number(value))
}
