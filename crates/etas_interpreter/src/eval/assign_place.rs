use super::*;
use crate::control::ExecutionFault;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LocalPlaceSegment {
    Field(String),
    Index(usize),
    MapKey(Box<InterpValue>),
}

#[derive(Clone, Debug)]
pub(crate) enum LocalPlaceComponent {
    Field(String),
    Index { base: HirExprId, index: HirExprId },
}

#[derive(Clone, Debug)]
pub(super) struct AssignTargetIndexResume {
    pub root_symbol: SymbolId,
    pub segments: Vec<LocalPlaceSegment>,
    pub components: Vec<LocalPlaceComponent>,
    pub next_component_index: usize,
    pub new_value: InterpValue,
    pub span: Span,
    pub frame: Frame,
    pub resume: Option<(HirBlockId, usize)>,
}

pub(super) struct FinishAssignTargetIndex {
    pub root_symbol: SymbolId,
    pub segments: Vec<LocalPlaceSegment>,
    pub components: Vec<LocalPlaceComponent>,
    pub next_component_index: usize,
    pub index_value: InterpValue,
    pub new_value: InterpValue,
    pub block: HirBlockId,
    pub next_stmt_index: usize,
    pub span: Span,
}

impl<'a> EvalContext<'a> {
    pub(super) fn resolve_local_place(
        &mut self,
        expr: HirExprId,
        frame: &mut Frame,
        span: Span,
        new_value: InterpValue,
        resume: Option<(HirBlockId, usize)>,
    ) -> Result<(SymbolId, Vec<LocalPlaceSegment>), Box<ControlSignal>> {
        let mut components = Vec::new();
        let root_symbol = match self.collect_local_place(expr, &mut components, span) {
            Ok(symbol) => symbol,
            Err(fault) => return Err(Box::new(ControlSignal::Fault(Box::new(fault)))),
        };
        self.resolve_local_place_components(
            AssignTargetIndexResume {
                root_symbol,
                segments: Vec::new(),
                components,
                next_component_index: 0,
                new_value,
                span,
                frame: frame.clone(),
                resume,
            },
            frame,
        )
    }

    fn collect_local_place(
        &mut self,
        expr: HirExprId,
        components: &mut Vec<LocalPlaceComponent>,
        span: Span,
    ) -> Result<SymbolId, ExecutionFault> {
        match &self.checked.hir.exprs[expr] {
            HirExpr::Path(path) => match path.resolution {
                ResolveResult::Resolved(symbol) => Ok(symbol),
                _ => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "assignment target path was not fully resolved",
                )),
            },
            HirExpr::Field { base, field, .. } => {
                let root = self.collect_local_place(*base, components, span)?;
                components.push(LocalPlaceComponent::Field(field.clone()));
                Ok(root)
            }
            HirExpr::Index { base, index, .. } => {
                let root = self.collect_local_place(*base, components, span)?;
                components.push(LocalPlaceComponent::Index {
                    base: *base,
                    index: *index,
                });
                Ok(root)
            }
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "assignment target must be a mutable local binding, field, list index, or map index",
            )),
        }
    }

    fn resolve_local_place_components(
        &mut self,
        mut state: AssignTargetIndexResume,
        frame: &mut Frame,
    ) -> Result<(SymbolId, Vec<LocalPlaceSegment>), Box<ControlSignal>> {
        for component_index in state.next_component_index..state.components.len() {
            let component = state.components[component_index].clone();
            match component {
                LocalPlaceComponent::Field(field) => {
                    state.segments.push(LocalPlaceSegment::Field(field));
                }
                LocalPlaceComponent::Index { base, index } => {
                    let index_value = match self.eval_expr(index, frame) {
                        ControlSignal::Value(value) => value,
                        signal @ (ControlSignal::Apply(_)
                        | ControlSignal::Checkpoint(_)
                        | ControlSignal::Block(_)
                        | ControlSignal::Expr(_)
                        | ControlSignal::Call(_)
                        | ControlSignal::Perform(_)
                        | ControlSignal::Memory(_)
                        | ControlSignal::Session(_)
                        | ControlSignal::Console(_)
                        | ControlSignal::Command(_)
                        | ControlSignal::Model(_)
                        | ControlSignal::Host(_)) => {
                            return Err(Box::new(self.assign_target_index_signal(
                                signal,
                                AssignTargetIndexResume {
                                    root_symbol: state.root_symbol,
                                    segments: state.segments,
                                    components: state.components,
                                    next_component_index: component_index + 1,
                                    new_value: state.new_value,
                                    span: state.span,
                                    frame: frame.clone(),
                                    resume: state.resume,
                                },
                            )));
                        }
                        ControlSignal::Return(value) => {
                            return Err(Box::new(ControlSignal::Return(value)));
                        }
                        ControlSignal::Resume(value) => {
                            return Err(Box::new(ControlSignal::Resume(value)));
                        }
                        ControlSignal::Finish(value) => {
                            return Err(Box::new(ControlSignal::Finish(value)));
                        }
                        ControlSignal::Break => return Err(Box::new(ControlSignal::Break)),
                        ControlSignal::Fault(fault) => {
                            return Err(Box::new(ControlSignal::Fault(fault)));
                        }
                        ControlSignal::Continue => return Err(Box::new(ControlSignal::Continue)),
                    };
                    if self.expr_type_is_map(base) {
                        state
                            .segments
                            .push(LocalPlaceSegment::MapKey(Box::new(index_value)));
                    } else {
                        let Some(index) = self.index_usize(index_value, state.span) else {
                            return Err(Box::new(ControlSignal::invalid_arguments(
                                "assignment index must be a non-negative integer",
                                state.span,
                            )));
                        };
                        state.segments.push(LocalPlaceSegment::Index(index));
                    }
                }
            }
        }
        Ok((state.root_symbol, state.segments))
    }

    fn expr_type_is_map(&self, expr: HirExprId) -> bool {
        self.plan.dispatch.is_map_expr(expr)
    }

    pub(super) fn assign_target_index_signal(
        &mut self,
        signal: ControlSignal,
        resume: AssignTargetIndexResume,
    ) -> ControlSignal {
        let Some((block, next_stmt_index)) = resume.resume else {
            return ControlSignal::runtime_fault(
                "assignment target index suspension requires statement continuation metadata",
                resume.span,
            );
        };
        let continuation = Continuation::AssignTargetIndex {
            block,
            next_stmt_index,
            root_symbol: resume.root_symbol,
            segments: resume.segments,
            components: resume.components,
            next_component_index: resume.next_component_index,
            new_value: resume.new_value,
            span: resume.span,
            frame: resume.frame,
        };
        compose_signal_continuation(signal, continuation)
    }

    pub(super) fn finish_assign_target_index(
        &mut self,
        mut resume: FinishAssignTargetIndex,
        frame: &mut Frame,
    ) -> Result<(), Box<ControlSignal>> {
        let Some(LocalPlaceComponent::Index { base, .. }) = resume
            .components
            .get(resume.next_component_index.saturating_sub(1))
        else {
            return Err(Box::new(ControlSignal::invalid_arguments(
                "assignment index continuation is missing its index component".to_owned(),
                resume.span,
            )));
        };
        if self.expr_type_is_map(*base) {
            resume
                .segments
                .push(LocalPlaceSegment::MapKey(Box::new(resume.index_value)));
        } else {
            let Some(index) = self.index_usize(resume.index_value, resume.span) else {
                return Err(Box::new(ControlSignal::invalid_arguments(
                    "assignment index must be a non-negative integer",
                    resume.span,
                )));
            };
            resume.segments.push(LocalPlaceSegment::Index(index));
        }
        let resolved = self.resolve_local_place_components(
            AssignTargetIndexResume {
                root_symbol: resume.root_symbol,
                segments: resume.segments,
                components: resume.components,
                next_component_index: resume.next_component_index,
                new_value: resume.new_value.clone(),
                span: resume.span,
                frame: frame.clone(),
                resume: Some((resume.block, resume.next_stmt_index)),
            },
            frame,
        )?;
        let (root_symbol, segments) = resolved;
        self.assign_resolved_local_place(
            root_symbol,
            &segments,
            resume.new_value,
            frame,
            resume.span,
        )
    }

    pub(super) fn index_usize(&self, value: InterpValue, _span: Span) -> Option<usize> {
        value.as_number()?.as_usize()
    }
}
