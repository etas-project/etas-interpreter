use etas_hir::HirExprId;
use etas_types::TypeId;

use crate::intrinsic::dispatch::CheckedStdIntrinsicCall;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentOperationKind {
    Step,
    Invoke,
}

impl AgentOperationKind {
    pub(crate) fn intrinsic(self) -> u32 {
        match self {
            Self::Step => etas_std::intrinsic::runtime::AGENT_STEP,
            Self::Invoke => etas_std::intrinsic::runtime::TOOL_INVOKE,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum AgentOperationOrigin {
    Standard(CheckedStdIntrinsicCall),
    Controller {
        inference: HirExprId,
        operation: AgentOperationKind,
    },
}

#[derive(Clone, Debug)]
pub struct CheckedAgentOperation {
    pub(crate) origin: AgentOperationOrigin,
    pub(crate) parameter_types: Vec<TypeId>,
    pub(crate) result_type: TypeId,
}

impl CheckedAgentOperation {
    pub(crate) fn standard(call: &CheckedStdIntrinsicCall) -> Self {
        Self {
            origin: AgentOperationOrigin::Standard(call.clone()),
            parameter_types: call.parameter_types.clone(),
            result_type: call.result_type,
        }
    }

    pub(crate) fn controller(
        checked: &etas_frontend::CheckedProject,
        inference: HirExprId,
        operation: AgentOperationKind,
    ) -> Result<Self, String> {
        let fact = checked
            .types
            .agent_controllers
            .get(&inference)
            .ok_or("default agent controller has no checked operation contracts")?;
        let operation_fact = match operation {
            AgentOperationKind::Step => &fact.step,
            AgentOperationKind::Invoke => &fact.invoke,
        };
        let symbol = checked
            .std_registry
            .symbol(operation_fact.target)
            .ok_or("agent operation declaration is missing")?;
        if symbol
            .intrinsic
            .as_ref()
            .is_none_or(|descriptor| descriptor.id.0 != operation.intrinsic())
        {
            return Err("agent operation differs from its canonical intrinsic".into());
        }
        Ok(Self {
            origin: AgentOperationOrigin::Controller {
                inference,
                operation,
            },
            parameter_types: operation_fact.signature.params.clone(),
            result_type: operation_fact.signature.output,
        })
    }
}
