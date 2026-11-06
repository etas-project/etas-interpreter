use std::{collections::BTreeSet, num::NonZeroUsize};

use super::{TaskId, TaskScopeId};

pub(super) struct TaskGroup {
    pub(super) parent: Option<TaskScopeId>,
    pub(super) concurrency: NonZeroUsize,
    pub(super) active: usize,
    pub(super) tasks: BTreeSet<TaskId>,
    pub(super) owner: Option<TaskId>,
    pub(super) waiters: Vec<TaskId>,
    pub(super) primary_failure: Option<TaskId>,
}

impl TaskGroup {
    pub(super) fn new(parent: Option<TaskScopeId>, concurrency: NonZeroUsize) -> Self {
        Self {
            parent,
            concurrency,
            active: 0,
            tasks: BTreeSet::new(),
            owner: None,
            waiters: Vec::new(),
            primary_failure: None,
        }
    }
}
