use super::*;
use crate::control::ExecutionFault;

macro_rules! eval_method_arg {
    ($result:expr) => {
        match $result {
            Ok(value) => value,
            Err(signal) => return *signal,
        }
    };
}

struct PromptMemorySelectionRequest {
    messages: Vec<crate::value::PromptMessage>,
    method: String,
    role: crate::value::PromptRole,
    allow_plain_system_content: bool,
    region_stable_id: String,
    path: Vec<String>,
    kind: crate::value::MemorySelectionKind,
    predicate: Option<InterpValue>,
    limit: Option<u32>,
    span: Span,
}

pub(super) struct MethodDispatch<'a> {
    pub expr: HirExprId,
    pub method: &'a str,
    pub type_args: &'a [etas_hir::HirTypeId],
    pub args: &'a [HirArg],
    pub span: Span,
}

pub(super) struct StaticMethodArgsState {
    pub expr: HirExprId,
    pub kind: StaticMethodKind,
    pub method: String,
    pub type_args: Vec<etas_hir::HirTypeId>,
    pub args: Vec<HirArg>,
    pub start_arg_index: usize,
    pub evaluated_args: Vec<InterpValue>,
    pub span: Span,
}

pub(super) struct LocalMethodArgsState {
    pub expr: HirExprId,
    pub receiver: InterpValue,
    pub method: String,
    pub type_args: Vec<etas_hir::HirTypeId>,
    pub args: Vec<HirArg>,
    pub start_arg_index: usize,
    pub evaluated_args: Vec<InterpValue>,
    pub span: Span,
}

mod advanced_collections;
mod args;
mod collections;
mod dispatch;
pub(super) mod helpers;
mod local;
mod message;
mod prompt;

use helpers::*;
