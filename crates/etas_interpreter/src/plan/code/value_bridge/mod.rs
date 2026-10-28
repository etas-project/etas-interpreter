use etas_code::descriptor::GenericOwner;
use etas_frontend::CheckedProgram;
use etas_types::{TrustWrapper, TypeId};
use std::sync::Arc;

use super::{RevisionTypeBridge, RevisionTypeBridgeError};
use crate::value::{InterpValue, RangeBounds, StringValue};

mod action;
mod aggregate;
mod callable;
pub(crate) use action::ActionProjection;
mod code;
pub(crate) use callable::CallableValueProjection;
pub(super) mod projection;
mod revision;
pub(crate) use revision::RevisionProjection;
mod sharing;
mod step;
#[cfg(test)]
mod tests;

#[derive(Debug)]
pub enum RevisionValueBridgeError {
    Type(RevisionTypeBridgeError),
    Value {
        source: TypeId,
        target: TypeId,
        failure: RevisionValueFailure,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevisionValueFailure {
    MissingType,
    MissingRepresentation,
    MissingEnumLayout,
    UnmaterializedType(String),
    InconsistentLayout,
    WrongValueKind {
        expected: &'static str,
        actual: &'static str,
    },
    WrongNominalIdentity {
        actual: TypeId,
    },
    WrongProgram,
    WrongArity {
        expected: usize,
        actual: usize,
    },
    InvalidPrimitive,
    InvalidField(String),
    UnknownVariant(String),
    ExecutableScopeRequired,
    InvalidCallable(String),
    UnsupportedType,
    CyclicType,
}

impl std::fmt::Display for RevisionValueBridgeError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "checked revision value conversion failed: {self:?}")
    }
}
impl std::error::Error for RevisionValueBridgeError {}

impl RevisionTypeBridge {
    pub(crate) fn value_to_candidate_scoped(
        self: &Arc<Self>,
        program: &CheckedProgram,
        scope: &GenericOwner,
        ty: TypeId,
        value: InterpValue,
        execution: &crate::value::CheckedExecutionProgram,
    ) -> Result<InterpValue, RevisionValueBridgeError> {
        let target = self
            .to_candidate(program, scope, ty)
            .map_err(RevisionValueBridgeError::Type)?;
        convert_scoped(
            program,
            self.candidate_program(),
            ty,
            target,
            value,
            Some(execution),
            Some(RevisionProjection::Candidate(self.clone())),
        )
    }

    pub(crate) fn value_to_original_scoped(
        self: &Arc<Self>,
        program: &CheckedProgram,
        scope: &GenericOwner,
        ty: TypeId,
        value: InterpValue,
        execution: &crate::value::CheckedExecutionProgram,
    ) -> Result<InterpValue, RevisionValueBridgeError> {
        let target = self
            .to_original(program, scope, ty)
            .map_err(RevisionValueBridgeError::Type)?;
        convert_scoped(
            program,
            self.original_program(),
            ty,
            target,
            value,
            Some(execution),
            Some(RevisionProjection::Original(self.clone())),
        )
    }

    pub fn value_to_candidate(
        &self,
        program: &CheckedProgram,
        scope: &GenericOwner,
        ty: TypeId,
        value: InterpValue,
    ) -> Result<InterpValue, RevisionValueBridgeError> {
        let target = self
            .to_candidate(program, scope, ty)
            .map_err(RevisionValueBridgeError::Type)?;
        convert(program, self.candidate_program(), ty, target, value)
    }

    pub fn value_to_original(
        &self,
        program: &CheckedProgram,
        scope: &GenericOwner,
        ty: TypeId,
        value: InterpValue,
    ) -> Result<InterpValue, RevisionValueBridgeError> {
        let target = self
            .to_original(program, scope, ty)
            .map_err(RevisionValueBridgeError::Type)?;
        convert(program, self.original_program(), ty, target, value)
    }
}

// Only the checked bridge chooses the root pair. Child pairs are projections of
// that frozen interface, never a structural-assignability test on caller IDs.
fn convert(
    source: &CheckedProgram,
    target: &CheckedProgram,
    from: TypeId,
    to: TypeId,
    value: InterpValue,
) -> Result<InterpValue, RevisionValueBridgeError> {
    convert_scoped(source, target, from, to, value, None, None)
}

fn convert_scoped(
    source: &CheckedProgram,
    target: &CheckedProgram,
    from: TypeId,
    to: TypeId,
    value: InterpValue,
    execution: Option<&crate::value::CheckedExecutionProgram>,
    revision: Option<RevisionProjection>,
) -> Result<InterpValue, RevisionValueBridgeError> {
    let mut walk = Walk {
        source,
        target,
        execution,
        revision,
        work: Vec::new(),
        values: Vec::new(),
        source_types: projection::CheckedValueTypes::default(),
        target_types: projection::CheckedValueTypes::default(),
        shared: std::collections::HashMap::new(),
    };
    walk.work.push(Work::Convert { from, to, value });
    while let Some(work) = walk.work.pop() {
        match work {
            Work::Convert { from, to, value } => {
                if let Some(key) = sharing::key(from, to, &value) {
                    if let Some(cached) = walk.shared.get(&key) {
                        walk.values.push(cached.target.clone());
                        continue;
                    }
                    walk.work.push(Work::Memoize {
                        key,
                        source: value.clone(),
                    });
                }
                walk.step(from, to, value)?;
            }
            Work::Memoize { key, source } => {
                let target = walk
                    .values
                    .last()
                    .ok_or_else(|| {
                        error(key.from, key.to, RevisionValueFailure::InconsistentLayout)
                    })?
                    .clone();
                walk.shared.insert(
                    key,
                    sharing::SharedConversion {
                        _source: source,
                        target,
                    },
                );
            }
            Work::Build {
                from,
                to,
                count,
                kind,
            } => {
                let start = walk
                    .values
                    .len()
                    .checked_sub(count)
                    .ok_or_else(|| error(from, to, RevisionValueFailure::InconsistentLayout))?;
                let values = walk.values.split_off(start);
                walk.values.push(
                    aggregate::build(kind, values).map_err(|failure| error(from, to, failure))?,
                );
            }
        }
    }
    if walk.values.len() != 1 {
        return Err(error(from, to, RevisionValueFailure::InconsistentLayout));
    }
    walk.values
        .pop()
        .ok_or_else(|| error(from, to, RevisionValueFailure::InconsistentLayout))
}

struct Walk<'a> {
    source: &'a CheckedProgram,
    target: &'a CheckedProgram,
    execution: Option<&'a crate::value::CheckedExecutionProgram>,
    revision: Option<RevisionProjection>,
    work: Vec<Work>,
    values: Vec<InterpValue>,
    source_types: projection::CheckedValueTypes,
    target_types: projection::CheckedValueTypes,
    shared: std::collections::HashMap<sharing::Key, sharing::SharedConversion>,
}

enum Work {
    Memoize {
        key: sharing::Key,
        source: InterpValue,
    },
    Convert {
        from: TypeId,
        to: TypeId,
        value: InterpValue,
    },
    Build {
        from: TypeId,
        to: TypeId,
        count: usize,
        kind: Build,
    },
}

enum Build {
    Nominal(TypeId),
    Trust(TrustWrapper),
    Array,
    List,
    Slice,
    Set,
    Map,
    Tuple,
    Record(Vec<String>),
    Variant(StringValue),
    Some,
    Message(crate::value::MessageValue),
    Range(RangeBounds),
}

impl Walk<'_> {
    fn children(
        &mut self,
        from: TypeId,
        to: TypeId,
        kind: Build,
        children: Vec<(TypeId, TypeId, InterpValue)>,
    ) {
        self.work.push(Work::Build {
            from,
            to,
            count: children.len(),
            kind,
        });
        self.work.extend(
            children
                .into_iter()
                .rev()
                .map(|(from, to, value)| Work::Convert { from, to, value }),
        );
    }
}

fn error(
    source: TypeId,
    target: TypeId,
    failure: RevisionValueFailure,
) -> RevisionValueBridgeError {
    RevisionValueBridgeError::Value {
        source,
        target,
        failure,
    }
}

fn wrong(expected: &'static str, value: &InterpValue) -> RevisionValueFailure {
    RevisionValueFailure::WrongValueKind {
        expected,
        actual: value.kind_name(),
    }
}
