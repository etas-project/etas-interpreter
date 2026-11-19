use etas_hir::HirExprId;
use etas_types::TypeId;

use crate::intrinsic::dispatch::CheckedStdIntrinsicCall;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentOperationKind {
    Step,
    Invoke,
}

impl AgentOperationKind {
    fn path(self) -> [&'static str; 3] {
        match self {
            Self::Step => ["std", "agent", "step"],
            Self::Invoke => ["std", "tool", "invoke"],
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum AgentOperationOrigin {
    Standard(CheckedStdIntrinsicCall),
    SourceTool(Box<super::CheckedActionInvocation>),
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
    pub(crate) fn source_tool(proof: super::CheckedActionInvocation) -> Result<Self, String> {
        let etas_effects::Effect::AppliedAction(request) = &proof.effect else {
            return Err("source tool invocation requires a checked agent context selector".into());
        };
        let registry = &proof.program.effect_registry;
        if registry.action_by_name("std.effects.Tool.call").as_ref() != Some(&request.action)
            || !matches!(request.args.as_slice(), [etas_types::EffectArgRef::Type(_)])
            || proof.parameters.len() != 3
        {
            return Err("tool invocation differs from the checked source Tool.call ABI".into());
        }
        Ok(Self {
            parameter_types: proof.parameters.clone(),
            result_type: proof.output,
            origin: AgentOperationOrigin::SourceTool(Box::new(proof)),
        })
    }

    pub(crate) fn tool_context(&self, checked: &etas_frontend::CheckedProject) -> Option<TypeId> {
        if let AgentOperationOrigin::SourceTool(proof) = &self.origin {
            let etas_effects::Effect::AppliedAction(request) = &proof.effect else {
                return None;
            };
            let [etas_types::EffectArgRef::Type(context)] = request.args.as_slice() else {
                return None;
            };
            return Some(*context);
        }
        let etas_types::Type::Applied { args, .. } = checked.type_store.get(self.result_type)?
        else {
            return None;
        };
        let [context] = args.as_slice() else {
            return None;
        };
        Some(*context)
    }

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
            .lookup_qualified(&operation.path())
            .ok_or("agent operation declaration is missing")?;
        if operation_fact.target != symbol.id {
            return Err("agent operation differs from its canonical declaration".into());
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
