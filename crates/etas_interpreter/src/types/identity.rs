use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
    sync::Arc,
};

use etas_types::{PrimitiveType, TrustWrapper};

/// A declaration in an authenticated checked compilation, not an instantiated
/// TypeStore index. Restore must resolve it through the current catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DeclarationIdentity {
    pub(super) compilation: [u8; 32],
    pub(super) declaration: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StaticArgument {
    Type(usize),
    Path(Vec<String>),
    String(String),
    Int(String),
    Wildcard,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EffectHead {
    pub name: String,
    pub arguments: Vec<StaticArgument>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum UnaryType {
    Array,
    List,
    Set,
    Range,
    Slice,
    Option,
    Schema,
    Message,
    MemorySelection,
    MemoryRegion,
    RegionHandle,
    ToolHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum BinaryType {
    Map,
    Result,
    Store,
}

/// Arguments contain type expressions only. Declaration members are deliberately
/// absent: following an ADT identity never unfolds its recursive definition.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TypeHead {
    Primitive(PrimitiveType),
    Declaration(DeclarationIdentity),
    Unary(UnaryType),
    Binary(BinaryType),
    Tuple,
    Record(Vec<String>),
    Function {
        inputs: usize,
        effects: Option<Vec<EffectHead>>,
    },
    Handler {
        handled: Vec<EffectHead>,
        produced: Vec<EffectHead>,
        has_result: bool,
    },
    Refined {
        compilation: [u8; 32],
        predicate: u32,
    },
    Trust(TrustWrapper),
    Prompt,
    PromptPart,
    MemoryPlace(Vec<String>),
    Resource(String),
}

struct Node {
    compilation: [u8; 32],
    head: TypeHead,
    arguments: Vec<CheckedTypeIdentity>,
    hash: u64,
}

/// Closed checked type expression. It never allocates an ID in the compiler's
/// frozen TypeStore; clone shares the expression, not a mutable type universe.
pub struct CheckedTypeIdentity(Option<Arc<Node>>);

impl CheckedTypeIdentity {
    pub(super) fn new(
        compilation: [u8; 32],
        head: TypeHead,
        arguments: Vec<Self>,
    ) -> Result<Self, String> {
        if arguments.iter().any(|arg| arg.compilation() != compilation) {
            return Err("type argument belongs to a different checked compilation".into());
        }
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        compilation.hash(&mut hash);
        head.hash(&mut hash);
        arguments.len().hash(&mut hash);
        for argument in &arguments {
            argument.hash(&mut hash);
        }
        Ok(Self(Some(Arc::new(Node {
            compilation,
            head,
            arguments,
            hash: hash.finish(),
        }))))
    }

    fn node(&self) -> &Node {
        self.0.as_deref().expect("live checked type expression")
    }

    pub(crate) fn head(&self) -> &TypeHead {
        &self.node().head
    }

    pub(super) fn compilation(&self) -> [u8; 32] {
        self.node().compilation
    }

    pub(crate) fn arguments(&self) -> &[Self] {
        &self.node().arguments
    }

    pub(crate) fn declaration(&self) -> Option<DeclarationIdentity> {
        match self.head() {
            TypeHead::Declaration(identity) => Some(*identity),
            _ => None,
        }
    }

    pub(super) fn pointer(&self) -> usize {
        Arc::as_ptr(self.0.as_ref().expect("live checked type expression")) as usize
    }
}

impl Clone for CheckedTypeIdentity {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl std::fmt::Debug for CheckedTypeIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CheckedTypeIdentity")
            .field("head", self.head())
            .field("argument_count", &self.arguments().len())
            .finish()
    }
}

impl Hash for CheckedTypeIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.node().hash.hash(state);
    }
}

impl PartialEq for CheckedTypeIdentity {
    fn eq(&self, other: &Self) -> bool {
        if self.pointer() == other.pointer() {
            return true;
        }
        if self.node().hash != other.node().hash {
            return false;
        }
        let mut pending = vec![(self, other)];
        let mut seen = HashSet::new();
        while let Some((left, right)) = pending.pop() {
            if left.pointer() == right.pointer() {
                continue;
            }
            if !seen.insert((left.pointer(), right.pointer())) {
                continue;
            }
            let (left, right) = (left.node(), right.node());
            if left.hash != right.hash
                || left.compilation != right.compilation
                || left.head != right.head
                || left.arguments.len() != right.arguments.len()
            {
                return false;
            }
            pending.extend(left.arguments.iter().zip(&right.arguments));
        }
        true
    }
}
impl Eq for CheckedTypeIdentity {}

impl Drop for CheckedTypeIdentity {
    fn drop(&mut self) {
        let Some(root) = self.0.take() else { return };
        let Ok(mut root) = Arc::try_unwrap(root) else {
            return;
        };
        let mut pending = std::mem::take(&mut root.arguments);
        while let Some(mut argument) = pending.pop() {
            let Some(node) = argument.0.take() else {
                continue;
            };
            if let Ok(mut node) = Arc::try_unwrap(node) {
                pending.append(&mut node.arguments);
            }
        }
    }
}
