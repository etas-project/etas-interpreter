use etas_hir::HirExprId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CallSiteKind {
    Expr { expr: HirExprId },
    Pipeline { expr: HirExprId },
    PipelineStage { expr: HirExprId, index: usize },
}

#[derive(Clone, Debug)]
pub struct CallSite {
    pub kind: CallSiteKind,
    pub frame: super::Frame,
    pub(crate) composition: Option<ComposedInvocation>,
}

#[derive(Clone, Debug)]
pub(crate) struct ComposedInvocation {
    pub root: super::CallTargetLink,
    pub path: InvocationPath,
}

// Persistent paths make nested composition entry and sibling advancement O(1).
// The explicit destructor also keeps deeply nested invocations off the Rust stack.
#[derive(Clone, Default)]
pub(crate) struct InvocationPath(Option<std::rc::Rc<PathNode>>);

#[derive(Debug)]
struct PathNode {
    parent: InvocationPath,
    index: usize,
}

impl InvocationPath {
    pub fn push(&self, index: usize) -> Self {
        Self(Some(std::rc::Rc::new(PathNode {
            parent: self.clone(),
            index,
        })))
    }

    pub fn advance(&self) -> Option<Self> {
        let node = self.0.as_ref()?;
        Some(node.parent.push(node.index.checked_add(1)?))
    }

    pub fn indices(&self) -> Vec<usize> {
        let mut indices = Vec::new();
        let mut node = self.0.as_deref();
        while let Some(current) = node {
            indices.push(current.index);
            node = current.parent.0.as_deref();
        }
        indices.reverse();
        indices
    }

    pub fn from_indices(indices: &[usize]) -> Self {
        indices
            .iter()
            .fold(Self::default(), |path, index| path.push(*index))
    }
}

impl Drop for InvocationPath {
    fn drop(&mut self) {
        let mut node = self.0.take();
        while let Some(current) = node {
            match std::rc::Rc::try_unwrap(current) {
                Ok(mut current) => node = current.parent.0.take(),
                Err(_) => break,
            }
        }
    }
}

impl std::fmt::Debug for InvocationPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.indices().fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::InvocationPath;

    #[test]
    fn invocation_paths_share_ancestors_and_release_without_recursive_drop() {
        let ancestor = InvocationPath::default().push(7);
        let mut path = ancestor.clone();
        for index in 0..50_000 {
            path = path.push(index);
        }
        let sibling = path.advance().unwrap();
        assert!(std::rc::Rc::ptr_eq(
            path.0.as_ref().unwrap().parent.0.as_ref().unwrap(),
            sibling.0.as_ref().unwrap().parent.0.as_ref().unwrap(),
        ));
        assert_eq!(sibling.indices().last(), Some(&50_000));
        drop(path);
        drop(sibling);
        assert_eq!(ancestor.indices(), [7]);
    }
}
