use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_if_expr(
        &mut self,
        cond: HirExprId,
        then_block: HirBlockId,
        else_branch: &Option<HirElseBranch>,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match self.eval_expr(cond, frame) {
            ControlSignal::Value(value) => {
                self.resume_if_expr(value, then_block, else_branch.clone(), span, frame)
            }
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
            | ControlSignal::Host(_)) => compose_signal_continuation(
                signal,
                Continuation::IfExpr {
                    then_block,
                    else_branch: else_branch.clone(),
                    span,
                    frame: frame.clone(),
                },
            ),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Continue => ControlSignal::Continue,
        }
    }

    pub(super) fn eval_match_expr(
        &mut self,
        scrutinee: HirExprId,
        arms: &[etas_hir::HirMatchArm],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match self.eval_expr(scrutinee, frame) {
            ControlSignal::Value(value) => self.resume_match_expr(value, arms, span, frame),
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
            | ControlSignal::Host(_)) => compose_signal_continuation(
                signal,
                Continuation::MatchExpr {
                    arms: arms.to_vec(),
                    span,
                    frame: frame.clone(),
                },
            ),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Continue => ControlSignal::Continue,
        }
    }

    pub(super) fn resume_if_expr(
        &mut self,
        cond_value: InterpValue,
        then_block: HirBlockId,
        else_branch: Option<HirElseBranch>,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let branch = match self.expect_bool(cond_value, span) {
            Ok(true) => return self.execute_block(then_block, frame),
            Ok(false) => else_branch,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        match branch {
            Some(HirElseBranch::If(expr)) => self.eval_expr(expr, frame),
            Some(HirElseBranch::Block(block)) => self.execute_block(block, frame),
            None => ControlSignal::Value(InterpValue::Unit),
        }
    }

    pub(super) fn resume_match_expr(
        &mut self,
        scrutinee: InterpValue,
        arms: &[etas_hir::HirMatchArm],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        self.execute_match_arms(scrutinee, arms, span, frame)
    }

    pub(super) fn resume_if_stmt(
        &mut self,
        resume: IfStmtResume,
        frame: &mut Frame,
    ) -> ControlSignal {
        let branch_signal = match self.expect_bool(resume.cond_value, resume.span) {
            Ok(true) => self.execute_block(resume.then_block, frame),
            Ok(false) => match resume.else_branch {
                Some(HirElseBranch::If(expr)) => self.eval_expr(expr, frame),
                Some(HirElseBranch::Block(block)) => self.execute_block(block, frame),
                None => ControlSignal::Value(InterpValue::Unit),
            },
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        match branch_signal {
            ControlSignal::Value(_) => {
                self.execute_block_from(resume.block, resume.next_stmt_index, frame)
            }
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
            | ControlSignal::Host(_)) => compose_signal_continuation(
                signal,
                Continuation::ContinueBlock {
                    block: resume.block,
                    next_stmt_index: resume.next_stmt_index,
                    frame: frame.clone(),
                },
            ),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Continue => ControlSignal::Continue,
        }
    }

    pub(super) fn resume_match_stmt(
        &mut self,
        scrutinee: InterpValue,
        block: HirBlockId,
        next_stmt_index: usize,
        arms: &[etas_hir::HirMatchArm],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match self.execute_match_arms(scrutinee, arms, span, frame) {
            ControlSignal::Value(_) => self.execute_block_from(block, next_stmt_index, frame),
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
            | ControlSignal::Host(_)) => compose_signal_continuation(
                signal,
                Continuation::ContinueBlock {
                    block,
                    next_stmt_index,
                    frame: frame.clone(),
                },
            ),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Continue => ControlSignal::Continue,
        }
    }

    fn execute_match_arms(
        &mut self,
        scrutinee: InterpValue,
        arms: &[etas_hir::HirMatchArm],
        span: Span,
        frame: &Frame,
    ) -> ControlSignal {
        for arm in arms {
            let mut arm_frame = frame.clone();
            match self.match_pattern(arm.pat, &scrutinee, &mut arm_frame, arm.span) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            }
            return match arm.body {
                etas_hir::HirMatchArmBody::Expr(expr) => self.eval_expr(expr, &mut arm_frame),
                etas_hir::HirMatchArmBody::Block(block) => {
                    self.execute_block(block, &mut arm_frame)
                }
            };
        }
        ControlSignal::runtime_fault("match reached runtime with no matching arm", span)
    }

    pub(super) fn expect_bool(
        &self,
        value: InterpValue,
        span: Span,
    ) -> Result<bool, ExecutionFault> {
        match value {
            InterpValue::Bool(value) => Ok(value),
            other => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("interpreter expected a boolean condition, got {:?}", other),
            )),
        }
    }
}

pub(super) struct IfStmtResume {
    pub cond_value: InterpValue,
    pub block: HirBlockId,
    pub next_stmt_index: usize,
    pub then_block: HirBlockId,
    pub else_branch: Option<HirElseBranch>,
    pub span: Span,
}
