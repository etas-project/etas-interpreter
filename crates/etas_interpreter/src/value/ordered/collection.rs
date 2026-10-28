use std::{cell::OnceCell, rc::Rc};

use super::{Cursor, InterpValue, OrderedTree, Search, SearchStep};
use crate::control::CallTarget;

#[derive(Debug)]
struct Storage {
    tree: OrderedTree,
    comparator: Option<CallTarget>,
    // Derived boundary views. Language iteration and lookup never use these caches.
    entries: OnceCell<Vec<(InterpValue, InterpValue)>>,
    keys: OnceCell<Vec<InterpValue>>,
}

impl Drop for Storage {
    fn drop(&mut self) {
        self.entries.take();
        self.keys.take();
        let tree = std::mem::take(&mut self.tree);
        for value in tree.into_values(self.comparator.take()) {
            super::super::shared::release_value(value);
        }
    }
}

#[derive(Clone, Debug)]
pub struct OrderedMapValue(Rc<Storage>);

#[derive(Clone, Debug)]
pub struct OrderedSetValue(Rc<Storage>);

impl Storage {
    fn new(tree: OrderedTree, comparator: Option<CallTarget>) -> Rc<Self> {
        Rc::new(Self {
            tree,
            comparator,
            entries: OnceCell::new(),
            keys: OnceCell::new(),
        })
    }
    fn entries(&self) -> &Vec<(InterpValue, InterpValue)> {
        self.entries
            .get_or_init(|| self.tree.iter().cloned().collect())
    }
    fn keys(&self) -> &Vec<InterpValue> {
        self.keys
            .get_or_init(|| self.tree.iter().map(|(key, _)| key.clone()).collect())
    }
}

macro_rules! common {
    ($name:ident) => {
        impl $name {
            pub fn new(comparator: Option<CallTarget>) -> Self {
                Self(Storage::new(OrderedTree::default(), comparator))
            }
            pub fn len(&self) -> usize {
                self.0.tree.len()
            }
            pub fn is_empty(&self) -> bool {
                self.0.tree.is_empty()
            }
            pub(crate) fn comparator(&self) -> Option<&CallTarget> {
                self.0.comparator.as_ref()
            }
            pub(crate) fn cursor(&self) -> Cursor {
                self.0.tree.cursor()
            }
            pub(crate) fn shared_capture_identity(&self) -> Option<*const ()> {
                (Rc::strong_count(&self.0) > 1).then(|| Rc::as_ptr(&self.0).cast())
            }
            pub(crate) fn search(self, key: InterpValue, value: Option<InterpValue>) -> Search {
                let tree = match Rc::try_unwrap(self.0) {
                    Ok(mut storage) => std::mem::take(&mut storage.tree),
                    Err(storage) => storage.tree.clone(),
                };
                tree.search(key, value)
            }
            pub(crate) fn from_tree(tree: OrderedTree, comparator: Option<CallTarget>) -> Self {
                Self(Storage::new(tree, comparator))
            }
            pub(crate) fn into_unique_values(self) -> Option<super::IntoValues> {
                Rc::try_unwrap(self.0).ok().map(|mut storage| {
                    std::mem::take(&mut storage.tree).into_values(storage.comparator.take())
                })
            }
        }
    };
}
common!(OrderedMapValue);
common!(OrderedSetValue);

impl OrderedMapValue {
    pub(crate) fn entry(&self, index: usize) -> Option<&(InterpValue, InterpValue)> {
        self.0.tree.entry(index)
    }
    pub fn borrow(&self) -> &Vec<(InterpValue, InterpValue)> {
        self.0.entries()
    }
    pub fn snapshot(&self) -> Vec<(InterpValue, InterpValue)> {
        self.borrow().clone()
    }
    pub(crate) fn try_from_entries(
        entries: Vec<(InterpValue, InterpValue)>,
    ) -> Result<Self, String> {
        Ok(Self::from_tree(build(entries)?, None))
    }
}

impl OrderedSetValue {
    pub(crate) fn key(&self, index: usize) -> Option<&InterpValue> {
        self.0.tree.entry(index).map(|entry| &entry.0)
    }
    pub fn borrow(&self) -> &[InterpValue] {
        self.0.keys()
    }
    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().to_vec()
    }
    pub(crate) fn try_from_values(values: Vec<InterpValue>) -> Result<Self, String> {
        for pair in values.windows(2) {
            if super::default_order(&pair[0], &pair[1])? == std::cmp::Ordering::Equal {
                return Err("duplicate set element in saved ordered set".into());
            }
        }
        Ok(Self::from_tree(
            build(
                values
                    .into_iter()
                    .map(|key| (key, InterpValue::Unit))
                    .collect(),
            )?,
            None,
        ))
    }
}

fn build(entries: Vec<(InterpValue, InterpValue)>) -> Result<OrderedTree, String> {
    for (key, _) in &entries {
        super::default_order(key, key)?;
    }
    for pair in entries.windows(2) {
        match super::default_order(&pair[0].0, &pair[1].0)? {
            std::cmp::Ordering::Less => (),
            std::cmp::Ordering::Equal => {
                return Err("duplicate ordered collection key in saved value".into());
            }
            std::cmp::Ordering::Greater => {
                return Err("saved ordered collection keys are not in ascending order".into());
            }
        }
    }
    let mut tree = OrderedTree::default();
    for (key, value) in entries {
        let mut search = tree.search(key, Some(value));
        let mut order = None;
        loop {
            match search.advance(order.take())? {
                SearchStep::Continue => (),
                SearchStep::Complete => break,
                SearchStep::Compare => {
                    let (a, b) = search.comparison()?;
                    order = Some(super::default_order(a, b)?);
                }
            }
        }
        tree = search.finish()?.tree;
    }
    Ok(tree)
}
