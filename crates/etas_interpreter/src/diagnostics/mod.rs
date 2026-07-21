use etas_core::{AnalysisDiagnosticCode, Diagnostic, SourceId, Span, TextSize};
use etas_frontend::CheckedProject;
use etas_hir::{HirItem, HirItemId};
use etas_types::{Type, TypeId};

use crate::{control::PendingPerform, value::InterpValue};

pub(crate) fn primary_span(project: &CheckedProject) -> Span {
    let source = project
        .sources
        .sources
        .first()
        .map(|source| source.id)
        .unwrap_or(SourceId(0));
    Span::empty(source, TextSize::ZERO)
}

pub(crate) fn item_span(project: &CheckedProject, item: HirItemId) -> Span {
    project
        .hir
        .items
        .get(item)
        .map(HirItem::span)
        .unwrap_or_else(|| primary_span(project))
}

pub(crate) fn analysis(
    code: AnalysisDiagnosticCode,
    span: Span,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic::analysis(code, span, message)
}

pub(crate) fn missing_checked_fact(span: Span, message: impl Into<String>) -> Diagnostic {
    analysis(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
}

pub(crate) fn invalid_arguments(span: Span, message: impl Into<String>) -> Diagnostic {
    analysis(AnalysisDiagnosticCode::InvalidArguments, span, message)
}

pub(crate) fn unhandled_runtime_error(span: Span, message: impl Into<String>) -> Diagnostic {
    analysis(AnalysisDiagnosticCode::UnhandledRuntimeError, span, message)
}

pub(crate) fn unhandled_error_perform(
    project: &CheckedProject,
    perform: &PendingPerform,
) -> Diagnostic {
    let error_name = perform
        .error_type
        .map(|error| error_type_name(project, error))
        .unwrap_or_else(|| "unknown".to_owned());
    let detail = runtime_error_detail(&perform.args);
    let message = match detail {
        Some(detail) => format!(
            "unhandled Error[{error_name}] escaped the checked-HIR interpreter entry: {detail}"
        ),
        None => format!("unhandled Error[{error_name}] escaped the checked-HIR interpreter entry"),
    };
    analysis(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        perform.span,
        message,
    )
}

pub(crate) fn unhandled_effect_action(perform: &PendingPerform) -> Diagnostic {
    analysis(
        AnalysisDiagnosticCode::UnhandledEffectAction,
        perform.span,
        format!(
            "unhandled effect action {} escaped the checked-HIR interpreter entry",
            effect_action_name(&perform.action)
        ),
    )
}

pub(crate) fn missing_entry(span: Span, message: impl Into<String>) -> Diagnostic {
    analysis(AnalysisDiagnosticCode::MissingEntry, span, message)
}

pub(crate) fn missing_host_handler(span: Span, message: impl Into<String>) -> Diagnostic {
    analysis(AnalysisDiagnosticCode::MissingHostHandler, span, message)
}

pub(crate) fn unsupported_phase2_runtime_feature(
    span: Span,
    message: impl Into<String>,
) -> Diagnostic {
    analysis(
        AnalysisDiagnosticCode::UnsupportedPhase2RuntimeFeature,
        span,
        message,
    )
}

fn error_type_name(project: &CheckedProject, error: TypeId) -> String {
    match project.type_store.get(error) {
        Some(Type::Named(named)) => named.name.clone(),
        Some(Type::Enum(named)) => named.name.clone(),
        Some(other) => format!("{other:?}"),
        None => format!("TypeId({})", error.0),
    }
}

fn runtime_error_detail(args: &[InterpValue]) -> Option<String> {
    runtime_error_detail_value(args.first()?)
}

fn runtime_error_detail_value(value: &InterpValue) -> Option<String> {
    let first = value;
    match first {
        InterpValue::Variant { fields, .. } => fields.iter().find_map(runtime_error_detail_value),
        InterpValue::String(message) => Some(message.clone()),
        other => Some(format!("{other:?}")),
    }
}

fn effect_action_name(action: &etas_hir::ResolvedActionRef) -> String {
    let owner = action
        .effect
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".");
    if owner.is_empty() {
        action.action.clone()
    } else {
        format!("{owner}.{}", action.action)
    }
}
