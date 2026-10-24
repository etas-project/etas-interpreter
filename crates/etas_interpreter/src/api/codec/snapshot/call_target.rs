use super::{Node, Pending, array};
use crate::api::codec::machine::{intrinsic_dispatch_name, runtime_limit_snapshot};
use crate::orchestration::{CallTargetSnapshot, LocalsSnapshot};
use serde_json::{Value, json};

pub(super) fn frame<'a>(frame: &'a LocalsSnapshot, slot: &'a mut Value, pending: &mut Pending<'a>) {
    *slot = json!({"id":frame.id, "locals":null,
        "type_bindings":frame.type_bindings.iter().map(|(name,ty)|json!({"name":name,"type":ty.0})).collect::<Vec<_>>()});
    for ((symbol, value), entry) in frame
        .locals
        .iter()
        .zip(array(&mut slot["locals"], frame.locals.len()))
    {
        *entry = json!({"symbol":symbol.0,"value":null});
        pending.push((Node::Value(value), &mut entry["value"]));
    }
}

pub(super) fn encode<'a>(
    target: &'a CallTargetSnapshot,
    slot: &'a mut Value,
    pending: &mut Pending<'a>,
) {
    use CallTargetSnapshot as T;
    match target {
        T::FlowItem(item) => *slot = json!({"kind":"flow","item":item.0}),
        T::AgentItem(item) => *slot = json!({"kind":"agent","item":item.0}),
        T::ToolItem(item) => *slot = json!({"kind":"tool","item":item.0}),
        T::SpecImplMethod(symbol) => *slot = json!({"kind":"spec_impl_method","symbol":symbol.0}),
        T::EnumVariant(symbol) => *slot = json!({"kind":"enum_variant","symbol":symbol.0}),
        T::NominalConstructor(ty) => *slot = json!({"kind":"nominal_constructor","ty":ty.0}),
        T::Lambda { expr, captured } => {
            *slot = json!({"kind":"lambda","expr":expr.0,"captured":null});
            pending.push((Node::Frame(captured), &mut slot["captured"]));
        }
        T::PureIntrinsic {
            intrinsic,
            parameter_types,
            result_type,
        } => {
            *slot = json!({
            "kind":"pure_intrinsic","intrinsic":intrinsic.0,"parameter_types":parameter_types.iter().map(|ty|ty.0).collect::<Vec<_>>(),"result_type":result_type.0})
        }
        T::StdIntrinsic {
            intrinsic,
            dispatch,
            parameter_types,
            result_type,
        } => {
            *slot = json!({
            "kind":"std_intrinsic","intrinsic":intrinsic.0,"dispatch":intrinsic_dispatch_name(*dispatch),"parameter_types":parameter_types.iter().map(|ty|ty.0).collect::<Vec<_>>(),"result_type":result_type.0})
        }
        T::Specialized {
            target,
            type_bindings,
        } => {
            *slot = json!({"kind":"specialized","target":null,"type_bindings":type_bindings.iter().map(|(name,ty)|json!({"name":name,"type":ty.0})).collect::<Vec<_>>()});
            pending.push((Node::CallTarget(target), &mut slot["target"]));
        }
        T::Limited { target, limits } => {
            *slot = json!({"kind":"limited","target":null,"limits":limits.iter().map(runtime_limit_snapshot).collect::<Vec<_>>()});
            pending.push((Node::CallTarget(target), &mut slot["target"]));
        }
        T::Composed(targets) => {
            *slot = json!({"kind":"composed","targets":null});
            pending.extend(
                targets
                    .iter()
                    .zip(array(&mut slot["targets"], targets.len()))
                    .map(|(target, slot)| (Node::CallTarget(target), slot)),
            );
        }
    }
}
