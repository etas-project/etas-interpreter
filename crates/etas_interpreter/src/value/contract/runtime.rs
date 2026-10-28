use super::{
    Context, Seed, Target, Value,
    callable::{self, Expected},
};
use crate::{control::GenericBindings, plan::InterpreterPlan, value::InterpValue};
use etas_frontend::CheckedProject;
use etas_types::{ItemSignature, SymbolTypeFact, TypeId};

struct RuntimeContext<'a> {
    checked: &'a CheckedProject,
    plan: &'a InterpreterPlan,
    limits: &'a etas_host::StorageLimits,
    snapshot: crate::eval::machine::snapshot::SnapshotValidator<'a>,
}

pub(crate) fn validate_arguments(
    checked: &CheckedProject,
    plan: &InterpreterPlan,
    item: etas_hir::HirItemId,
    values: &[InterpValue],
    limits: &etas_host::StorageLimits,
) -> Result<(), String> {
    let signature = match checked.types.item_signatures.get(&item) {
        Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => Some(signature),
        Some(ItemSignature::Agent(agent)) => agent.callable(),
        _ => None,
    }
    .ok_or("entry has no checked callable signature")?;
    if signature.params.len() != values.len() {
        return Err(format!(
            "entry expects {} argument(s) but received {}",
            signature.params.len(),
            values.len()
        ));
    }
    validate_values(checked, plan, values, &signature.params, limits)
}

pub(crate) fn validate_value(
    checked: &CheckedProject,
    plan: &InterpreterPlan,
    value: &InterpValue,
    ty: TypeId,
    limits: &etas_host::StorageLimits,
) -> Result<(), String> {
    validate_values(checked, plan, std::slice::from_ref(value), &[ty], limits)
}

fn validate_values(
    checked: &CheckedProject,
    plan: &InterpreterPlan,
    values: &[InterpValue],
    types: &[TypeId],
    limits: &etas_host::StorageLimits,
) -> Result<(), String> {
    if values.len() != types.len() {
        return Err("runtime values do not match the checked type vector length".into());
    }
    let context = RuntimeContext {
        checked,
        plan,
        limits,
        snapshot: crate::eval::machine::snapshot::SnapshotValidator::new(
            checked,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            limits,
        ),
    };
    super::validate(
        &context,
        values.iter().zip(types).map(|(value, ty)| Seed {
            value: Value::Runtime(value),
            ty: *ty,
        }),
        &GenericBindings::new(),
    )?;

    // A frame is borrowed while its selected captures are checked. Discover
    // nested frames iteratively, without cloning captured payloads or snapshots.
    let mut discovery = crate::value::graph::FrameDiscovery::default();
    let mut frames = Vec::new();
    for value in values {
        frames.extend(
            discovery
                .captures(crate::value::graph::Node::Value(value))
                .into_iter()
                .map(|(expr, frame)| (expr, frame, false)),
        );
    }
    let mut active = std::collections::HashSet::new();
    let mut completed = std::collections::HashSet::new();
    while let Some((expr, frame, complete)) = frames.pop() {
        let key = (expr, frame.capture_identity());
        if complete {
            active.remove(&key);
            completed.insert(key);
            continue;
        }
        if completed.contains(&key) {
            continue;
        }
        if !active.insert(key) {
            return Err("input closure has cyclic captured frames".into());
        }
        frames.push((expr, frame.clone(), true));
        let mut discovery = crate::value::graph::FrameDiscovery::default();
        let layout = plan
            .closures
            .get(expr)
            .ok_or("input closure has no checked capture layout")?;
        let locals = frame.borrow_locals()?;
        let locals = locals
            .sorted()
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let mut seeds = Vec::with_capacity(layout.captures.len());
        for symbol in layout.captures.iter() {
            let value = *locals
                .get(symbol)
                .ok_or_else(|| format!("input closure is missing captured symbol {symbol:?}"))?;
            let ty = match checked.types.symbol_types.get(symbol) {
                Some(
                    SymbolTypeFact::Param { ty }
                    | SymbolTypeFact::Local { ty, .. }
                    | SymbolTypeFact::TopLevelLet { ty, .. }
                    | SymbolTypeFact::Value { ty }
                    | SymbolTypeFact::Field { ty },
                ) => *ty,
                _ => return Err("input closure capture has no checked value type".into()),
            };
            seeds.push(Seed {
                value: Value::Runtime(value),
                ty,
            });
            frames.extend(
                discovery
                    .captures(crate::value::graph::Node::Value(value))
                    .into_iter()
                    .map(|(expr, frame)| (expr, frame, false)),
            );
        }
        super::validate(&context, seeds, frame.generic_bindings())?;
    }
    Ok(())
}

impl Context for RuntimeContext<'_> {
    fn checked(&self) -> &CheckedProject {
        self.checked
    }
    fn limits(&self) -> &etas_host::StorageLimits {
        self.limits
    }
    fn callable(&self, value: Target<'_>, expected: TypeId) -> Result<(), String> {
        self.target(value, Expected::Value(expected))
    }
    fn comparator(&self, value: Target<'_>, key: TypeId) -> Result<(), String> {
        self.target(value, Expected::Comparison(key))
    }
    fn native(&self, value: Value<'_>, expected: TypeId) -> Result<(), String> {
        let known = &self.checked.types.known_std_types;
        let constructor = match self.checked.type_store.get(expected) {
            Some(etas_types::Type::Applied { constructor, .. }) => TypeId(constructor.0),
            _ => expected,
        };
        match value {
            Value::Runtime(InterpValue::HostHandle(handle))
                if handle.nominal_type() == expected
                    && !known.runtime_values.values().any(|ty| *ty == expected)
                    && known.opaque_values.contains(&constructor) =>
            {
                Ok(())
            }
            Value::Runtime(InterpValue::CodeMetadata(value)) if value.ty() == expected => {
                value.validate_role(&known.code)
            }
            Value::Runtime(InterpValue::SyntaxBlock(value))
                if value.ty() == expected && known.code.syntax_block == Some(expected) =>
            {
                Ok(())
            }
            Value::Runtime(InterpValue::AgentDefinition(value))
                if value.ty() == expected
                    && known.code.agent_definition == Some(constructor)
                    && std::sync::Arc::ptr_eq(value.program(), self.checked) =>
            {
                Ok(())
            }
            Value::Runtime(InterpValue::CallableDefinition(value))
                if value.ty() == expected
                    && known.code.callable_definition == Some(expected)
                    && std::sync::Arc::ptr_eq(value.program(), self.checked) =>
            {
                Ok(())
            }
            Value::Runtime(InterpValue::Handler {
                fact_expr,
                handlers,
            }) => self
                .snapshot
                .handler_value_contract(*fact_expr, handlers, expected),
            _ => Err("native input has a different checked program or type identity".into()),
        }
    }
}

impl RuntimeContext<'_> {
    fn target(&self, target: Target<'_>, expected: Expected) -> Result<(), String> {
        let Target::Runtime(target) = target else {
            return Err("runtime input contains a snapshot callable".into());
        };
        callable::validate(self.checked, self.plan, target, expected)
    }
}
