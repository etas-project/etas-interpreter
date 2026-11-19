//! Runtime primitives for checked source actions. Language handlers have
//! already had the opportunity to consume the request before this boundary.
use super::*;
use etas_effects::{
    CONSOLE_STDERR_WRITE_ACTION, CONSOLE_STDIN_READ_ALL_ACTION, CONSOLE_STDIN_READ_LINE_ACTION,
    CONSOLE_STDOUT_WRITE_ACTION, CONSOLE_TAG,
};
use etas_host::console::{ConsoleOperation, ConsoleRequest};
use etas_types::{EffectArgRef, PrimitiveType, Type};

impl EvalContext<'_> {
    pub(crate) fn source_compiler_default(
        &self,
        perform: &PendingPerform,
        role: etas_std::CompilerActionRole,
    ) -> bool {
        let (Some(proof), Some(signature)) = (
            perform.checked_action.as_ref(),
            self.checked
                .effect_registry
                .compiler_action(&self.checked.std_registry, role),
        ) else {
            return false;
        };
        let action = etas_effects::ActionRef {
            tag: signature.owner,
            action: signature.id,
        };
        self.checked.hir.source_standard_types
            && std::sync::Arc::ptr_eq(&proof.program, self.checked)
            && signature.has_default_handler
            && self.plan.action_mediation.has_default_action(&action)
            && match &proof.effect {
                etas_effects::Effect::Action(actual) => actual == &action,
                etas_effects::Effect::AppliedAction(actual) => actual.action == action,
                _ => false,
            }
    }

    pub(crate) fn source_host_action(&mut self, perform: &PendingPerform) -> Option<ControlSignal> {
        self.source_console_action(perform)
            .or_else(|| self.source_filesystem_action(perform))
            .or_else(|| self.source_network_action(perform))
            .or_else(|| self.source_browser_action(perform))
            .or_else(|| self.source_secret_action(perform))
            .or_else(|| self.source_command_action(perform))
            .or_else(|| self.source_session_action(perform))
            .or_else(|| self.source_memory_action(perform))
            .or_else(|| self.source_agent_step(perform))
            .or_else(|| self.source_tool_action(perform))
    }

    pub(in crate::eval) fn source_default_signature<'a>(
        &'a self,
        perform: &PendingPerform,
    ) -> Option<(&'a str, &'a str)> {
        let proof = perform.checked_action.as_ref()?;
        let action = match &proof.effect {
            etas_effects::Effect::Action(action) => action,
            etas_effects::Effect::AppliedAction(instance) => &instance.action,
            _ => return None,
        };
        if !self.checked.hir.source_standard_types
            || !std::sync::Arc::ptr_eq(&proof.program, self.checked)
            || !self.plan.action_mediation.has_default_action(action)
        {
            return None;
        }
        let registry = &self.checked.effect_registry;
        let signature = registry.action_signature(action)?;
        if !signature.has_default_handler {
            return None;
        }
        let owner = registry.tag_name(action.tag)?.rsplit('.').next()?;
        let name = signature.name.as_str();
        // Actions introduced in the source SDK can have session-local ids.
        // Only the action under the reserved std package supplies the default.
        if registry.standard_action(action.action).is_none()
            && registry
                .action_by_name(&format!("std.effects.{owner}.{name}"))
                .as_ref()
                != Some(action)
        {
            return None;
        }
        Some((owner, name))
    }

    pub(crate) fn source_console_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let proof = perform.checked_action.as_ref()?;
        let etas_effects::Effect::Action(action) = &proof.effect else {
            return None;
        };
        if action.tag != CONSOLE_TAG {
            return None;
        }
        if !self
            .checked
            .effect_registry
            .action_signature(action)
            .is_some_and(|signature| signature.has_default_handler)
        {
            return None;
        }
        let (parameter, output, decode) = match action.action {
            CONSOLE_STDOUT_WRITE_ACTION | CONSOLE_STDERR_WRITE_ACTION => (
                Some(PrimitiveType::String),
                PrimitiveType::Unit,
                ConsoleDecode::Unit,
            ),
            CONSOLE_STDIN_READ_ALL_ACTION | CONSOLE_STDIN_READ_LINE_ACTION => {
                (None, PrimitiveType::String, ConsoleDecode::String)
            }
            _ => return None,
        };
        let checked = &self.checked;
        let contract = || -> Result<(), String> {
            if !std::sync::Arc::ptr_eq(&proof.program, checked)
                || !self.plan.action_mediation.has_default_action(action)
            {
                return Err("Console default is absent from the checked mediation plan".into());
            }
            if proof
                .parameters
                .iter()
                .map(|ty| checked.type_store.get(*ty).cloned())
                .ne(parameter.into_iter().map(|ty| Some(Type::Primitive(ty))))
                || checked.type_store.get(proof.output) != Some(&Type::Primitive(output))
            {
                return Err(
                    "Console action signature differs from the runtime primitive ABI".into(),
                );
            }
            let expr = perform
                .expr
                .ok_or("Console default has no checked source origin")?;
            let Some(HirExpr::Perform { action, .. }) = checked.hir.exprs.get(expr) else {
                return Err("Console default has no checked perform expression".into());
            };
            let ResolveResult::Resolved(symbol) = action.action_symbol else {
                return Err("Console action declaration is unresolved".into());
            };
            let row = checked
                .types
                .action_signatures
                .get(&symbol)
                .and_then(|signature| signature.default_handler_effects.as_ref())
                .ok_or("Console action has no declared default Effect row")?;
            let [effect] = row.effects.as_slice() else {
                return Err("Console runtime default requires exactly Error<IOError>".into());
            };
            if row.tail.is_some()
                || checked.effect_registry.tag_by_name(&effect.name)
                    != Some(etas_effects::ERROR_TAG)
                || !matches!(effect.args.as_slice(), [EffectArgRef::Type(ty)] if Some(*ty) == self.known_std_types.io_error)
            {
                return Err("Console runtime default requires exactly Error<IOError>".into());
            }
            Ok(())
        };
        if let Err(reason) = contract() {
            return Some(ControlSignal::missing_checked_fact(reason, perform.span));
        }
        let operation = match (action.action, perform.args.as_slice()) {
            (CONSOLE_STDOUT_WRITE_ACTION, [InterpValue::String(text)]) => {
                ConsoleOperation::WriteStdout {
                    text: text.to_string(),
                    newline: false,
                }
            }
            (CONSOLE_STDERR_WRITE_ACTION, [InterpValue::String(text)]) => {
                ConsoleOperation::WriteStderr {
                    text: text.to_string(),
                    newline: false,
                }
            }
            (CONSOLE_STDIN_READ_ALL_ACTION, []) => ConsoleOperation::ReadAllStdin,
            (CONSOLE_STDIN_READ_LINE_ACTION, []) => ConsoleOperation::ReadLineStdin,
            _ => {
                return Some(ControlSignal::missing_checked_fact(
                    "Console payload differs from the checked action ABI",
                    perform.span,
                ));
            }
        };
        Some(ControlSignal::Console(Box::new(PendingConsole {
            request: ConsoleRequest {
                id: self.next_host_request_id(),
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            decode,
            span: perform.span,
            continuation: perform.continuation.clone(),
        })))
    }
}
