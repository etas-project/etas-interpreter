//! Persistent ordered storage. Comparisons are supplied by the checked evaluator.
use std::{cmp::Ordering, rc::Rc};

use super::InterpValue;
mod collection;
mod order;
pub use collection::{OrderedMapValue, OrderedSetValue};
pub(crate) use order::default_order;
pub(crate) use order::snapshot_order;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default)]
pub struct OrderedTree {
    root: Link,
}

type Link = Option<Rc<Node>>;

#[derive(Debug)]
struct Node {
    entry: (InterpValue, InterpValue),
    left: Link,
    right: Link,
    height: u32,
    count: usize,
}

impl Clone for Node {
    fn clone(&self) -> Self {
        #[cfg(test)]
        tests::record_copy();
        Self {
            entry: self.entry.clone(),
            left: self.left.clone(),
            right: self.right.clone(),
            height: self.height,
            count: self.count,
        }
    }
}

impl Node {
    fn leaf(entry: (InterpValue, InterpValue)) -> Rc<Self> {
        Rc::new(Self {
            entry,
            left: None,
            right: None,
            height: 1,
            count: 1,
        })
    }

    fn refresh(&mut self) {
        self.height = 1 + height(&self.left).max(height(&self.right));
        self.count = 1 + count(&self.left) + count(&self.right);
    }

    fn balance(&self) -> i64 {
        i64::from(height(&self.left)) - i64::from(height(&self.right))
    }
}

fn height(link: &Link) -> u32 {
    link.as_ref().map_or(0, |node| node.height)
}

fn count(link: &Link) -> usize {
    link.as_ref().map_or(0, |node| node.count)
}

#[derive(Clone, Copy, Debug)]
enum Direction {
    Left,
    Right,
}

fn child(node: &mut Node, direction: Direction) -> &mut Link {
    match direction {
        Direction::Left => &mut node.left,
        Direction::Right => &mut node.right,
    }
}

fn rotate(mut root: Rc<Node>, direction: Direction) -> Result<Rc<Node>, String> {
    let opposite = match direction {
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
    };
    let Some(mut promoted) = child(Rc::make_mut(&mut root), opposite).take() else {
        return Err("ordered tree rotation is missing its promoted child".into());
    };
    let middle = child(Rc::make_mut(&mut promoted), direction).take();
    *child(Rc::make_mut(&mut root), opposite) = middle;
    Rc::make_mut(&mut root).refresh();
    *child(Rc::make_mut(&mut promoted), direction) = Some(root);
    Rc::make_mut(&mut promoted).refresh();
    Ok(promoted)
}

fn rebalance(mut root: Rc<Node>) -> Result<Rc<Node>, String> {
    Rc::make_mut(&mut root).refresh();
    if root.balance() > 1 {
        if root.left.as_ref().is_some_and(|node| node.balance() < 0) {
            let left = Rc::make_mut(&mut root)
                .left
                .take()
                .ok_or("ordered tree has no left child")?;
            Rc::make_mut(&mut root).left = Some(rotate(left, Direction::Left)?);
        }
        rotate(root, Direction::Right)
    } else if root.balance() < -1 {
        if root.right.as_ref().is_some_and(|node| node.balance() > 0) {
            let right = Rc::make_mut(&mut root)
                .right
                .take()
                .ok_or("ordered tree has no right child")?;
            Rc::make_mut(&mut root).right = Some(rotate(right, Direction::Right)?);
        }
        rotate(root, Direction::Left)
    } else {
        Ok(root)
    }
}

impl OrderedTree {
    pub(crate) fn from_pending_entries(
        entries: Vec<(InterpValue, InterpValue)>,
    ) -> Result<Self, String> {
        struct BuildFrame {
            right_count: usize,
            left: Link,
            entry: Option<(InterpValue, InterpValue)>,
        }
        let mut count = entries.len();
        let mut entries = entries.into_iter();
        let mut parents = Vec::new();
        loop {
            while count != 0 {
                parents.push(BuildFrame {
                    right_count: count - count / 2 - 1,
                    left: None,
                    entry: None,
                });
                count /= 2;
            }
            let mut subtree = None;
            loop {
                let Some(parent) = parents.last_mut() else {
                    if entries.next().is_some() {
                        return Err("ordered restore left unconsumed entries".into());
                    }
                    return Ok(Self { root: subtree });
                };
                if parent.entry.is_none() {
                    parent.left = subtree;
                    parent.entry = Some(
                        entries
                            .next()
                            .ok_or("ordered restore lost a median entry")?,
                    );
                    count = parent.right_count;
                    break;
                }
                let parent = parents.pop().ok_or("ordered restore lost its ancestor")?;
                let mut node = Node {
                    entry: parent
                        .entry
                        .ok_or("ordered restore lost its median entry")?,
                    left: parent.left,
                    right: subtree,
                    height: 0,
                    count: 0,
                };
                node.refresh();
                subtree = Some(Rc::new(node));
            }
        }
    }
    pub fn len(&self) -> usize {
        count(&self.root)
    }
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn entry(&self, mut index: usize) -> Option<&(InterpValue, InterpValue)> {
        let mut current = self.root.as_deref();
        while let Some(node) = current {
            let before = count(&node.left);
            match index.cmp(&before) {
                Ordering::Less => current = node.left.as_deref(),
                Ordering::Equal => return Some(&node.entry),
                Ordering::Greater => {
                    index -= before + 1;
                    current = node.right.as_deref();
                }
            }
        }
        None
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            pending: Vec::new(),
            next: self.root.as_deref(),
            remaining: self.len(),
        }
    }

    pub fn cursor(&self) -> Cursor {
        Cursor {
            pending: Vec::new(),
            next: self.root.clone(),
            remaining: self.len(),
        }
    }

    pub fn search(self, key: InterpValue, insert: Option<InterpValue>) -> Search {
        Search {
            current: self.root,
            parents: Vec::new(),
            key,
            writing: insert.is_some(),
            insert,
            found: None,
            stage: Stage::Descend,
            awaiting_comparison: false,
        }
    }

    fn into_values(self, comparator: Option<crate::control::CallTarget>) -> IntoValues {
        IntoValues {
            pending: self.root.into_iter().collect(),
            value: None,
            comparator,
        }
    }
}

pub(crate) struct IntoValues {
    pending: Vec<Rc<Node>>,
    value: Option<InterpValue>,
    comparator: Option<crate::control::CallTarget>,
}

impl Iterator for IntoValues {
    type Item = InterpValue;
    fn next(&mut self) -> Option<Self::Item> {
        if self.value.is_some() {
            return self.value.take();
        }
        while let Some(node) = self.pending.pop() {
            if let Ok(node) = Rc::try_unwrap(node) {
                self.pending.extend(node.left);
                self.pending.extend(node.right);
                self.value = Some(node.entry.1);
                return Some(node.entry.0);
            }
        }
        self.comparator.take().map(InterpValue::Callable)
    }
}

pub struct Iter<'a> {
    pending: Vec<&'a Node>,
    next: Option<&'a Node>,
    remaining: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a (InterpValue, InterpValue);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.next.take() {
            self.next = node.left.as_deref();
            self.pending.push(node);
        }
        let node = self.pending.pop()?;
        self.next = node.right.as_deref();
        self.remaining -= 1;
        Some(&node.entry)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for Iter<'_> {}

/// An owning in-order cursor retains one immutable version, not a flat snapshot.
#[derive(Clone, Debug)]
pub struct Cursor {
    pending: Vec<Rc<Node>>,
    next: Link,
    remaining: usize,
}

impl Iterator for Cursor {
    type Item = (InterpValue, InterpValue);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.next.take() {
            self.next = node.left.clone();
            self.pending.push(node);
        }
        let node = self.pending.pop()?;
        self.next = node.right.clone();
        self.remaining -= 1;
        Some(node.entry.clone())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for Cursor {}

#[derive(Clone, Copy, Debug)]
enum Stage {
    Descend,
    Unwind,
    Done,
}

#[derive(Clone, Debug)]
pub struct Search {
    current: Link,
    parents: Vec<(Rc<Node>, Direction)>,
    key: InterpValue,
    insert: Option<InterpValue>,
    writing: bool,
    found: Option<InterpValue>,
    stage: Stage,
    awaiting_comparison: bool,
}

pub enum SearchStep {
    Continue,
    Compare,
    Complete,
}

pub struct SearchResult {
    pub tree: OrderedTree,
    pub found: Option<InterpValue>,
}

impl Search {
    pub(crate) fn key(&self) -> &InterpValue {
        &self.key
    }

    pub fn comparison(&self) -> Result<(&InterpValue, &InterpValue), String> {
        if !self.awaiting_comparison {
            return Err("ordered search has no pending comparison".into());
        }
        let node = self
            .current
            .as_ref()
            .ok_or("ordered comparison has no current node")?;
        Ok((&self.key, &node.entry.0))
    }

    pub fn advance(&mut self, order: Option<Ordering>) -> Result<SearchStep, String> {
        if self.awaiting_comparison {
            let order = order.ok_or("ordered search requires its comparison result")?;
            self.awaiting_comparison = false;
            let mut node = self
                .current
                .take()
                .ok_or("ordered search lost its current node")?;
            if order == Ordering::Equal {
                self.found = Some(node.entry.1.clone());
                if let Some(value) = self.insert.take() {
                    Rc::make_mut(&mut node).entry.1 = value;
                }
                self.current = Some(node);
                self.stage = Stage::Unwind;
            } else if self.insert.is_some() {
                let direction = if order == Ordering::Less {
                    Direction::Left
                } else {
                    Direction::Right
                };
                self.current = child(Rc::make_mut(&mut node), direction).take();
                self.parents.push((node, direction));
            } else {
                self.current = if order == Ordering::Less {
                    node.left.clone()
                } else {
                    node.right.clone()
                };
                // Read-only searches retain the original root without copying nodes.
                self.parents.push((
                    node,
                    if order == Ordering::Less {
                        Direction::Left
                    } else {
                        Direction::Right
                    },
                ));
            }
            return Ok(SearchStep::Continue);
        }
        if order.is_some() {
            return Err("ordered search received an unsolicited comparison".into());
        }
        match self.stage {
            Stage::Descend => {
                if self.current.is_some() {
                    self.awaiting_comparison = true;
                    Ok(SearchStep::Compare)
                } else {
                    if let Some(value) = self.insert.take() {
                        self.current = Some(Node::leaf((self.key.clone(), value)));
                    }
                    self.stage = Stage::Unwind;
                    Ok(SearchStep::Continue)
                }
            }
            Stage::Unwind => {
                if let Some((mut node, direction)) = self.parents.pop() {
                    if self.writing {
                        *child(Rc::make_mut(&mut node), direction) = self.current.take();
                        self.current = Some(rebalance(node)?);
                    } else {
                        self.current = Some(node);
                    }
                    Ok(SearchStep::Continue)
                } else {
                    self.stage = Stage::Done;
                    Ok(SearchStep::Complete)
                }
            }
            Stage::Done => Err("ordered search was already completed".into()),
        }
    }

    pub fn finish(self) -> Result<SearchResult, String> {
        if !matches!(self.stage, Stage::Done) {
            return Err("ordered search has not completed".into());
        }
        Ok(SearchResult {
            tree: OrderedTree { root: self.current },
            found: self.found,
        })
    }
}
