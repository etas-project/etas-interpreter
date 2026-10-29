use super::*;

pub(super) fn write<'a>(
    target: &'a CallTarget,
    slot: &'a mut Value,
    mut child: impl FnMut(Node<'a>, &'a mut Value),
) {
    match target {
        CallTarget::FlowItem(item) => *slot = json!({"kind":"flow","item":item.0}),
        CallTarget::AgentItem(item) => *slot = json!({"kind":"agent","item":item.0}),
        CallTarget::AgentRevision(value) => {
            *slot = json!({"kind":"compiled_agent_display", "identity":value.prepared().identity(), "revision":value.prepared().revision().key().fingerprint().to_string(), "persistent":false})
        }
        CallTarget::Scoped(_) => {
            *slot = json!({"kind":"scoped_callable_display","persistent":false})
        }
        CallTarget::ToolItem(item) => *slot = json!({"kind":"tool","item":item.0}),
        CallTarget::EnumVariant(symbol) => *slot = json!({"kind":"enum_variant","symbol":symbol.0}),
        CallTarget::NominalConstructor(ty) => {
            *slot = json!({"kind":"nominal_constructor","ty":ty.0})
        }
        CallTarget::Lambda { expr, captured } => {
            *slot = json!({"kind":"lambda","expr":expr.0,"captured":null});
            child(Node::Frame(captured), &mut slot["captured"]);
        }
        CallTarget::PureIntrinsic(call) => {
            *slot = json!({"kind":"pure_intrinsic","intrinsic":call.intrinsic.0,"parameter_types":call.parameter_types.iter().map(|ty|ty.0).collect::<Vec<_>>(),"result_type":call.result_type.0})
        }
        CallTarget::StdIntrinsic(call) => {
            *slot = json!({"kind":"std_intrinsic","origin":call.origin.0,"substitutions":machine::intrinsic_substitutions_snapshot(&call.substitutions),"intrinsic":call.identity.intrinsic.0,"dispatch":machine::intrinsic_dispatch_name(call.identity.dispatch),"parameter_types":call.parameter_types.iter().map(|ty|ty.0).collect::<Vec<_>>(),"result_type":call.result_type.0})
        }
        CallTarget::Specialized {
            target,
            type_bindings,
            effect_row_bindings,
        } => {
            *slot = json!({"kind":"specialized","effect_row_bindings":effect_row_bindings,"target":null,"type_bindings":type_bindings.iter().map(|(name,ty)|json!({"name":name,"type":ty.0})).collect::<Vec<_>>()});
            child(Node::Target(target), &mut slot["target"]);
        }
        CallTarget::Limited { target, limits } => {
            *slot = json!({"kind":"limited","target":null,"limits":limits.iter().map(machine::runtime_limit_snapshot).collect::<Vec<_>>()});
            child(Node::Target(target), &mut slot["target"]);
        }
        CallTarget::Composed(targets) => {
            *slot = json!({"kind":"composed","targets":null});
            for (target, slot) in targets
                .iter()
                .zip(array(&mut slot["targets"], targets.len()))
            {
                child(Node::Target(target), slot);
            }
        }
    }
}
