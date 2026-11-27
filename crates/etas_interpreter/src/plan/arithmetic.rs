//! Register evaluation of bounded, straight-line numeric HIR. Calls, branches
//! and observable operations stay in the ordinary continuation machine.
use std::collections::{HashMap, HashSet};

use etas_frontend::{CheckedProject, OptimizationLevel};
use etas_hir::{
    HirBinaryOp, HirExpr, HirExprId, HirItem, HirItemId, HirLiteral, HirPat, HirStmt,
    ResolveResult, SymbolId,
};
use etas_types::{ItemSignature, PrimitiveType, Type};

use crate::value::{InterpValue, NumericValue};

const MAX_NODES: usize = 128;

#[derive(Debug)]
enum Instruction {
    Literal(NumericValue),
    Binary(HirBinaryOp, usize, usize),
}

#[derive(Debug)]
enum Output {
    Number(usize),
    Tuple(Vec<usize>),
}

#[derive(Debug)]
pub(crate) struct ArithmeticBody {
    inputs: Vec<PrimitiveType>,
    instructions: Vec<Instruction>,
    output: Output,
}

pub(super) type NumericLiterals = HashMap<HirExprId, (etas_types::TypeId, NumericValue)>;

pub(super) fn numeric_literals(project: &CheckedProject) -> NumericLiterals {
    project
        .hir
        .exprs
        .iter()
        .filter_map(|(expr, value)| {
            let HirExpr::Literal(literal) = value else {
                return None;
            };
            let ty = project.types.expression_producer_type(expr)?;
            let kind = match project.type_store.get(ty)? {
                Type::Primitive(kind) => *kind,
                Type::IntegerLiteral { .. } => PrimitiveType::I32,
                _ => return None,
            };
            let number = match literal {
                HirLiteral::Int { text, .. } => NumericValue::parse_integer(text, kind).ok()?,
                HirLiteral::Float { text, .. } => NumericValue::parse_float(text, kind).ok()?,
                _ => return None,
            };
            Some((expr, (ty, number)))
        })
        .collect()
}

pub(super) fn prepare(
    project: &CheckedProject,
    literals: &NumericLiterals,
) -> HashMap<HirItemId, ArithmeticBody> {
    if !project
        .compile_options
        .optimization_level
        .enables(OptimizationLevel::O2)
    {
        return HashMap::new();
    }
    let agent_methods: HashSet<_> = project
        .types
        .agent_methods
        .values()
        .map(|method| method.body)
        .collect();
    project
        .hir
        .items
        .iter()
        .filter_map(|(id, item)| {
            let HirItem::Flow(flow) = item else {
                return None;
            };
            let ItemSignature::Flow(signature) = project.types.item_signatures.get(&id)? else {
                return None;
            };
            if agent_methods.contains(&id)
                || project.hir.item_annotations.contains_key(&id)
                || flow.params.len() != signature.params.len()
                || !signature.generic_params.is_empty()
                || !flow.type_params.is_empty()
                || flow.receiver_mode != etas_hir::HirReceiverMode::None
                || signature.params.len() > MAX_NODES
            {
                return None;
            }
            let inputs = flow
                .params
                .iter()
                .zip(&signature.params)
                .map(|(symbol, ty)| {
                    let Type::Primitive(kind) = project.type_store.get(*ty)? else {
                        return None;
                    };
                    Some((*symbol, *kind))
                })
                .collect::<Option<Vec<_>>>()?;
            let block = &project.hir.blocks[flow.body.as_block()?];
            let mut builder = Builder {
                project,
                literals,
                locals: inputs
                    .iter()
                    .enumerate()
                    .map(|(i, (symbol, _))| (*symbol, i))
                    .collect(),
                input_len: inputs.len(),
                instructions: Vec::new(),
                nodes: 0,
            };
            let (last, lets) = match block.final_expr {
                Some(value) => (value, block.stmts.as_slice()),
                None => {
                    let (last, prior) = block.stmts.split_last()?;
                    let HirStmt::Return {
                        value: Some(value), ..
                    } = project.hir.stmts[*last]
                    else {
                        return None;
                    };
                    (value, prior)
                }
            };
            for stmt in lets {
                let HirStmt::Let { pat, value, .. } = project.hir.stmts[*stmt] else {
                    return None;
                };
                let HirPat::Binding { symbol, .. } = project.hir.pats[pat] else {
                    return None;
                };
                let index = builder.expr(value)?;
                builder.locals.insert(symbol, index);
            }
            if project.types.expression_producer_type(last) != Some(signature.output)
                || project.types.existential_packs.contains_key(&last)
            {
                return None;
            }
            let output = match &project.hir.exprs[last] {
                HirExpr::Tuple { elems, .. } => Output::Tuple(
                    elems
                        .iter()
                        .map(|e| builder.expr(*e))
                        .collect::<Option<_>>()?,
                ),
                _ => Output::Number(builder.expr(last)?),
            };
            if builder.instructions.is_empty() {
                return None;
            }
            Some((
                id,
                ArithmeticBody {
                    inputs: inputs.into_iter().map(|(_, kind)| kind).collect(),
                    instructions: builder.instructions,
                    output,
                },
            ))
        })
        .collect()
}

struct Builder<'a> {
    project: &'a CheckedProject,
    literals: &'a NumericLiterals,
    locals: HashMap<SymbolId, usize>,
    input_len: usize,
    instructions: Vec<Instruction>,
    nodes: usize,
}

impl Builder<'_> {
    fn expr(&mut self, expr: HirExprId) -> Option<usize> {
        self.nodes += 1;
        if self.nodes > MAX_NODES || self.project.types.existential_packs.contains_key(&expr) {
            return None;
        }
        if !matches!(
            self.project
                .type_store
                .get(self.project.types.expression_producer_type(expr)?),
            Some(Type::Primitive(_) | Type::IntegerLiteral { .. })
        ) {
            return None;
        }
        let instruction = match &self.project.hir.exprs[expr] {
            HirExpr::Path(path) => {
                let ResolveResult::Resolved(symbol) = path.resolution else {
                    return None;
                };
                return self.locals.get(&symbol).copied();
            }
            HirExpr::Literal(_) => Instruction::Literal(self.literals.get(&expr)?.1),
            HirExpr::Binary { op, lhs, rhs, .. }
                if matches!(
                    op,
                    HirBinaryOp::Add
                        | HirBinaryOp::Sub
                        | HirBinaryOp::Mul
                        | HirBinaryOp::Div
                        | HirBinaryOp::Rem
                        | HirBinaryOp::BitXor
                ) =>
            {
                Instruction::Binary(*op, self.expr(*lhs)?, self.expr(*rhs)?)
            }
            _ => return None,
        };
        let register = self.input_len + self.instructions.len();
        self.instructions.push(instruction);
        Some(register)
    }
}

impl ArithmeticBody {
    pub(crate) fn evaluate(&self, args: &[InterpValue]) -> Option<InterpValue> {
        let mut registers = Vec::with_capacity(self.inputs.len() + self.instructions.len());
        if args.len() != self.inputs.len() {
            return None;
        }
        for (kind, arg) in self.inputs.iter().zip(args) {
            let InterpValue::Number(input) = arg else {
                return None;
            };
            if input.primitive() != *kind {
                return None;
            }
            registers.push(*input);
        }
        for instruction in &self.instructions {
            let result = match *instruction {
                Instruction::Literal(value) => value,
                Instruction::Binary(op, a, b) => {
                    let (a, b) = (registers[a], registers[b]);
                    match op {
                        HirBinaryOp::Add => a.checked_add(b),
                        HirBinaryOp::Sub => a.checked_sub(b),
                        HirBinaryOp::Mul => a.checked_mul(b),
                        HirBinaryOp::Div => a.checked_div(b),
                        HirBinaryOp::Rem => a.checked_rem(b),
                        HirBinaryOp::BitXor => a.bit_xor(b),
                        _ => unreachable!(),
                    }
                    .ok()?
                }
            };
            registers.push(result);
        }
        Some(match &self.output {
            Output::Number(i) => InterpValue::Number(registers[*i]),
            Output::Tuple(indices) => InterpValue::Tuple(
                indices
                    .iter()
                    .map(|i| InterpValue::Number(registers[*i]))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        })
    }

    pub(crate) fn work(&self) -> u64 {
        self.instructions.len() as u64
    }
}
