use etas_effects::Effect;
use etas_types::TypeId;

use crate::value::CheckedExecutionProgram;

mod source;
mod standard;

pub(crate) use source::source_action;
pub(crate) use standard::standard_action;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckedActionAbi {
    pub effect: Effect,
    pub parameters: Vec<TypeId>,
    pub output: TypeId,
    pub returns_never: bool,
}

impl CheckedActionAbi {
    pub(crate) fn project(
        &self,
        edge: &crate::plan::code::RevisionProjection,
    ) -> Result<Self, String> {
        Self::project_parts(
            &self.effect,
            &self.parameters,
            self.output,
            self.returns_never,
            edge,
        )
    }

    pub(crate) fn project_parts(
        effect: &Effect,
        parameters: &[TypeId],
        output: TypeId,
        returns_never: bool,
        edge: &crate::plan::code::RevisionProjection,
    ) -> Result<Self, String> {
        let effect = match effect {
            Effect::Action(action) => Effect::Action(edge.action(action)?),
            Effect::AppliedAction(instance) => {
                Effect::AppliedAction(etas_effects::ActionInstanceRef {
                    action: edge.action(&instance.action)?,
                    args: instance
                        .args
                        .iter()
                        .map(|arg| match arg {
                            etas_types::EffectArgRef::Type(ty) => {
                                edge.runtime_type(*ty).map(etas_types::EffectArgRef::Type)
                            }
                            arg => Ok(arg.clone()),
                        })
                        .collect::<Result<_, String>>()?,
                })
            }
            _ => return Err("resumable action requires a closed canonical action instance".into()),
        };
        Ok(Self {
            effect,
            parameters: parameters
                .iter()
                .map(|ty| edge.runtime_type(*ty))
                .collect::<Result<_, _>>()?,
            output: edge.runtime_type(output)?,
            returns_never,
        })
    }

    pub(super) fn require_mediation(
        &self,
        execution: &CheckedExecutionProgram,
    ) -> Result<(), String> {
        let action = match &self.effect {
            Effect::Action(action) => action,
            Effect::AppliedAction(instance) => &instance.action,
            _ => return Err("action has no closed checked action identity".into()),
        };
        if !execution.plan.action_mediation.requires_action(action) {
            return Err("action invocation has no checked request/payload contract".into());
        }
        Ok(())
    }
}
