use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn is_checkpoint_callee(&self, callee: HirExprId) -> bool {
        let HirExpr::Path(path) = &self.checked.hir.exprs[callee] else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        self.checked.symbols.get(symbol).is_some_and(|symbol| {
            matches!(
                &symbol.def,
                SymbolDef::ImportAlias { path, .. }
                    if path.iter().map(String::as_str).eq(
                        ["std", "runtime", "checkpoint"].iter().copied()
                    ) || path.iter().map(String::as_str).eq(
                        ["std", "runtime", "checkpoint", "checkpoint"].iter().copied()
                    )
            )
        })
    }

    pub(super) fn checkpoint_label(
        &mut self,
        expr: HirExprId,
        frame: &mut Frame,
    ) -> Option<Option<String>> {
        let HirExpr::Call { callee, args, .. } = &self.checked.hir.exprs[expr] else {
            return None;
        };
        if !self.is_checkpoint_callee(*callee) {
            return None;
        }
        let label = args.first().and_then(|arg| {
            let value = match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            };
            match self.checked.hir.exprs.get(value) {
                Some(HirExpr::Literal(HirLiteral::String { value, .. })) => Some(value.clone()),
                _ => None,
            }
        });
        let _ = frame;
        Some(label)
    }

    pub(crate) fn execute_from_checkpoint_signal(
        &mut self,
        checkpoint: &InterpreterCheckpoint,
    ) -> ControlSignal {
        self.host_context = checkpoint.host_context.clone();
        self.handler_stack = checkpoint.handlers.handlers.clone();
        self.retry_stack = checkpoint.retry_state.attempts.clone();
        self.completed_host_boundaries = checkpoint.completed_host_boundaries.completed.clone();
        self.resource_versions = checkpoint.resource_versions.versions.clone();
        self.next_checkpoint = checkpoint.id.0 + 1;
        self.next_retry = self
            .retry_stack
            .iter()
            .map(|attempt| attempt.id.0 + 1)
            .max()
            .unwrap_or(0);
        self.next_handler_scope = self
            .handler_stack
            .iter()
            .map(|handler| handler.id.0 + 1)
            .max()
            .unwrap_or(0);
        self.next_step = checkpoint.trace.events_recorded as u32;
        self.next_host_request = self.completed_host_boundaries.len() as u32;
        self.next_message = checkpoint.trace.next_message;
        ControlSignal::Value(InterpValue::Unit)
    }

    pub(super) fn record_checkpoint(
        &mut self,
        label: Option<String>,
        machine: crate::orchestration::MachineSnapshot,
    ) -> Result<(), crate::control::ExecutionFault> {
        let id = CheckpointId(self.next_checkpoint);
        let compilation = crate::orchestration::CheckpointCompilationIdentity::for_project(
            self.checked,
            self.entry_item,
        )
        .map_err(|message| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                item_span(self.checked, self.entry_item),
                format!("checkpoint compilation identity is unavailable: {message}"),
            )
        })?;
        self.next_checkpoint += 1;
        self.events.push(WorkflowEvent::CheckpointCreated(id));
        self.checkpoints.push(InterpreterCheckpoint {
            id,
            label,
            compilation,
            entry_item: self.entry_item,
            args: self.entry_args.to_vec(),
            machine,
            handlers: HandlerSnapshot {
                handlers: self.handler_stack.clone(),
            },
            retry_state: RetrySnapshot {
                attempts: self.retry_stack.clone(),
            },
            trace: TraceSnapshot {
                events_recorded: self.events.len(),
                next_message: self.next_message,
            },
            host_context: self.host_context.clone(),
            current_session: self.current_session.clone(),
            resource_versions: ResourceVersionSnapshot {
                versions: self.resource_versions.clone(),
            },
            completed_host_boundaries: HostBoundaryLedger {
                completed: self.completed_host_boundaries.clone(),
            },
        });
        Ok(())
    }

    pub(super) fn step_id(&mut self) -> WorkflowStepId {
        let id = WorkflowStepId(self.next_step);
        self.next_step += 1;
        id
    }
}
