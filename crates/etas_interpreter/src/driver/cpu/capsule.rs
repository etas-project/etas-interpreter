use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::value::{ArrayValue, BytesValue, InterpValue, RecordValue, SharedFields, SharedValue};

use super::{CpuJobId, CpuTaskCompletion};

#[derive(Clone, Debug)]
pub(crate) struct PreparedCpuProgram {
    pub(crate) checked: etas_frontend::CheckedProject,
    pub(crate) plan: Arc<crate::api::InterpreterPlan>,
}

impl PreparedCpuProgram {
    pub(crate) fn new(
        checked: etas_frontend::CheckedProject,
        plan: Arc<crate::api::InterpreterPlan>,
    ) -> Self {
        #[cfg(test)]
        PROGRAM_BUILDS.set(PROGRAM_BUILDS.get() + 1);
        Self { checked, plan }
    }
}

#[cfg(test)]
std::thread_local! { static PROGRAM_BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
pub(crate) fn program_build_count() -> u64 {
    PROGRAM_BUILDS.get()
}

#[derive(Clone, Debug)]
pub(crate) struct CpuBudgetLease {
    pub(crate) steps: Arc<std::sync::atomic::AtomicU64>,
}

/// The parent's immutable run contract. Host context includes the shared wall
/// time/token/cost ledger; execution is rebound to the admitted child scope.
#[derive(Clone, Debug)]
pub(crate) struct CpuRuntimeContract {
    pub(crate) execution_limits: crate::api::ExecutionLimits,
    pub(crate) limit_span: etas_core::Span,
    pub(crate) entry_item: etas_hir::HirItemId,
    pub(crate) storage_limits: etas_host::StorageLimits,
    pub(crate) syntax_limits: etas_frontend::code::service::NativeSyntaxLimits,
    pub(crate) host_context: crate::api::HostExecutionContext,
    pub(crate) model_policy: crate::api::ModelExecutionPolicy,
    pub(crate) execution: etas_host::execution::ExecutionScope,
    pub(crate) replay_keys: crate::eval::replay::RunReplayKeys,
}

#[derive(Clone, Debug)]
pub(crate) struct CpuTaskCapsule {
    pub(crate) job: CpuJobId,
    pub(crate) dispatch_sequence: u64,
    pub(crate) program: Arc<PreparedCpuProgram>,
    pub(crate) callable: super::FrozenCpuCallable,
    pub(crate) arguments: Vec<FrozenValue>,
    pub(crate) budget: CpuBudgetLease,
    pub(crate) contract: CpuRuntimeContract,
    pub(crate) cancellation: Arc<AtomicBool>,
    pub(crate) call_span: etas_core::Span,
    pub(crate) timings: crate::api::CpuTaskTimings,
    pub(crate) submitted: Option<std::time::Instant>,
    #[cfg(test)]
    pub(crate) inject_panic: bool,
}

impl CpuTaskCapsule {
    pub(crate) fn assign_job(&mut self, job: CpuJobId) {
        self.job = job;
    }

    pub(crate) fn execute(self, worker: crate::orchestration::CpuWorkerId) -> CpuTaskCompletion {
        #[cfg(test)]
        if self.inject_panic {
            panic!("injected CPU task panic");
        }
        crate::eval::cpu::execute(self, worker)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrozenValue {
    Unit,
    Bool(bool),
    Number(crate::value::NumericValue),
    String(String),
    Bytes(Vec<u8>),
    Tuple(Vec<FrozenValue>),
    Array(Vec<FrozenValue>),
    Record(Vec<(String, FrozenValue)>),
    Variant {
        name: String,
        fields: Vec<FrozenValue>,
    },
    OptionNone,
    OptionSome(Box<FrozenValue>),
}

impl FrozenValue {
    pub(crate) fn freeze(value: &InterpValue) -> Option<Self> {
        Some(match value {
            InterpValue::Unit => Self::Unit,
            InterpValue::Bool(value) => Self::Bool(*value),
            InterpValue::Number(value) => Self::Number(*value),
            InterpValue::String(value) => Self::String(value.clone().into_string()),
            InterpValue::Bytes(value) => Self::Bytes(value.clone().into_vec()),
            InterpValue::Tuple(values) => Self::Tuple(
                values
                    .iter()
                    .map(Self::freeze)
                    .collect::<Option<Vec<_>>>()?,
            ),
            InterpValue::Array(values) => Self::Array(
                values
                    .borrow()
                    .iter()
                    .map(Self::freeze)
                    .collect::<Option<Vec<_>>>()?,
            ),
            InterpValue::Record(record) => Self::Record(
                record
                    .borrow()
                    .iter()
                    .map(|(name, value)| Some((name.clone(), Self::freeze(value)?)))
                    .collect::<Option<Vec<_>>>()?,
            ),
            InterpValue::Variant { name, fields } => Self::Variant {
                name: name.clone().into_string(),
                fields: fields
                    .iter()
                    .map(Self::freeze)
                    .collect::<Option<Vec<_>>>()?,
            },
            InterpValue::OptionNone => Self::OptionNone,
            InterpValue::OptionSome(value) => {
                Self::OptionSome(Box::new(Self::freeze(value.as_ref())?))
            }
            _ => return None,
        })
    }

    pub(crate) fn thaw(self) -> InterpValue {
        match self {
            Self::Unit => InterpValue::Unit,
            Self::Bool(value) => InterpValue::Bool(value),
            Self::Number(value) => InterpValue::Number(value),
            Self::String(value) => InterpValue::String(value.into()),
            Self::Bytes(value) => InterpValue::Bytes(BytesValue::from(value)),
            Self::Tuple(values) => InterpValue::Tuple(SharedFields::new(
                values.into_iter().map(Self::thaw).collect(),
            )),
            Self::Array(values) => InterpValue::Array(ArrayValue::new(
                values.into_iter().map(Self::thaw).collect(),
            )),
            Self::Record(fields) => InterpValue::Record(RecordValue::new(
                fields
                    .into_iter()
                    .map(|(name, value)| (name, value.thaw()))
                    .collect(),
            )),
            Self::Variant { name, fields } => InterpValue::Variant {
                name: name.into(),
                fields: SharedFields::new(fields.into_iter().map(Self::thaw).collect()),
            },
            Self::OptionNone => InterpValue::OptionNone,
            Self::OptionSome(value) => InterpValue::OptionSome(SharedValue::new(value.thaw())),
        }
    }
}
