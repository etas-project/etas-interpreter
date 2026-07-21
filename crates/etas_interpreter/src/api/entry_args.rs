use etas_frontend::CheckedProject;
use etas_hir::HirItem;
use etas_types::{ItemSignature, PrimitiveType, Type};

use crate::value::InterpValue;

pub fn default_entry_args(checked: &CheckedProject) -> Vec<InterpValue> {
    entry_args_from_strings(checked, Vec::new())
}

pub fn entry_args_from_strings(checked: &CheckedProject, args: Vec<String>) -> Vec<InterpValue> {
    let Some(entry) = checked.entry else {
        return Vec::new();
    };
    let Some(HirItem::Flow(_)) = checked.hir.items.get(entry) else {
        return Vec::new();
    };
    let Some(ItemSignature::Flow(signature)) = checked.types.item_signatures.get(&entry) else {
        return Vec::new();
    };

    match signature.params.as_slice() {
        [] => Vec::new(),
        [param] if is_string_array_type(checked, *param) => {
            vec![InterpValue::Array(
                args.into_iter()
                    .map(InterpValue::String)
                    .collect::<Vec<_>>()
                    .into(),
            )]
        }
        _ => Vec::new(),
    }
}

pub fn entry_requires_console(checked: &CheckedProject) -> bool {
    let Some(entry) = checked.entry else {
        return false;
    };
    let support = checked
        .interpreter_support
        .entry
        .as_ref()
        .or_else(|| checked.interpreter_support.items.get(&entry));
    support.is_some_and(interpreter_support_requires_console)
}

fn interpreter_support_requires_console(support: &etas_effects::InterpreterSupport) -> bool {
    match support {
        etas_effects::InterpreterSupport::LocalOnly
        | etas_effects::InterpreterSupport::Rejected(_) => false,
        etas_effects::InterpreterSupport::RequiresHost(requirements) => requirements
            .kinds
            .contains(&etas_effects::HostRequirementKind::Console),
        etas_effects::InterpreterSupport::RequiresInterpreterOrchestration(requirements) => {
            requirements
                .host
                .kinds
                .contains(&etas_effects::HostRequirementKind::Console)
        }
    }
}

fn is_string_array_type(checked: &CheckedProject, ty: etas_types::TypeId) -> bool {
    match checked.type_store.get(ty) {
        Some(Type::Array(inner)) => matches!(
            checked.type_store.get(*inner),
            Some(Type::Primitive(PrimitiveType::String))
        ),
        _ => false,
    }
}
