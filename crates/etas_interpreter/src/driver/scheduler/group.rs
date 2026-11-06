use std::{collections::BTreeSet, num::NonZeroUsize};

use super::{TaskId, TaskScopeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskGroupPolicy {
    Run,
    JoinTuple,
    JoinArray,
    TryJoin,
    Collect,
    Race,
    Map,
    MapCollect,
}

pub(super) struct TaskGroup {
    pub(super) parent: Option<TaskScopeId>,
    pub(super) concurrency: NonZeroUsize,
    pub(super) policy: TaskGroupPolicy,
    pub(super) active: usize,
    pub(super) tasks: BTreeSet<TaskId>,
    pub(super) owner: Option<TaskId>,
    pub(super) waiters: Vec<TaskId>,
    pub(super) primary_failure: Option<TaskId>,
    pub(super) winner: Option<TaskId>,
}

impl TaskGroup {
    pub(super) fn new(
        parent: Option<TaskScopeId>,
        concurrency: NonZeroUsize,
        policy: TaskGroupPolicy,
    ) -> Self {
        Self {
            parent,
            concurrency,
            policy,
            active: 0,
            tasks: BTreeSet::new(),
            owner: None,
            waiters: Vec::new(),
            primary_failure: None,
            winner: None,
        }
    }
}
