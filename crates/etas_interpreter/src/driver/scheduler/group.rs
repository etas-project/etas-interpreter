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

impl TaskGroupPolicy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::JoinTuple => "join_tuple",
            Self::JoinArray => "join_array",
            Self::TryJoin => "try_join",
            Self::Collect => "collect",
            Self::Race => "race",
            Self::Map => "map_concurrent",
            Self::MapCollect => "map_concurrent_collect",
        }
    }
}

pub(super) struct TaskGroup {
    pub(super) parent: Option<TaskScopeId>,
    pub(super) concurrency: NonZeroUsize,
    pub(super) serialize_host_actions: bool,
    pub(super) guard_resources: bool,
    pub(super) serial_task_admission: bool,
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
            serialize_host_actions: false,
            guard_resources: false,
            serial_task_admission: false,
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
