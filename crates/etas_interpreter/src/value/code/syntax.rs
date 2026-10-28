use std::{fmt, sync::Arc};

use etas_code::syntax::{CodeSyntaxOperations, SyntaxBlockRef, SyntaxOperationError};

/// Keeps the originating structured-syntax backend alive. A digest or source
/// string cannot be substituted for this lease.
#[derive(Clone)]
pub struct SyntaxBlockValue(Arc<SyntaxBlockData>);

struct SyntaxBlockData {
    ty: etas_types::TypeId,
    syntax: SyntaxBlockRef,
    backend: Arc<dyn CodeSyntaxOperations>,
}

impl SyntaxBlockValue {
    pub(crate) fn new(
        ty: etas_types::TypeId,
        syntax: SyntaxBlockRef,
        backend: Arc<dyn CodeSyntaxOperations>,
    ) -> Self {
        Self(Arc::new(SyntaxBlockData {
            ty,
            syntax,
            backend,
        }))
    }

    pub(crate) fn ty(&self) -> etas_types::TypeId {
        self.0.ty
    }

    pub(crate) fn syntax(&self) -> &SyntaxBlockRef {
        &self.0.syntax
    }
    pub(crate) fn in_revision(&self, ty: etas_types::TypeId) -> Self {
        Self::new(ty, self.0.syntax.clone(), self.0.backend.clone())
    }

    pub(crate) fn format_controlled(
        &self,
        max_output_bytes: usize,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<String, SyntaxOperationError> {
        self.0
            .backend
            .format_bounded_controlled(&self.0.syntax, max_output_bytes, control)
    }
}

impl fmt::Debug for SyntaxBlockValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyntaxBlockValue")
            .field("ty", &self.0.ty)
            .field("identity", &self.0.syntax.identity())
            .finish_non_exhaustive()
    }
}

impl PartialEq for SyntaxBlockValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.ty == other.0.ty
            && self.0.syntax.identity() == other.0.syntax.identity()
            && Arc::ptr_eq(&self.0.backend, &other.0.backend)
    }
}
impl Eq for SyntaxBlockValue {}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_frontend::code::service::NativeCodeSyntaxOperations;

    struct Continue;
    impl etas_utils::PipelineExecutionControl for Continue {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Ok(())
        }
    }

    #[test]
    fn syntax_value_keeps_its_backend_alive_and_applies_the_current_format_ceiling() {
        assert_eq!(size_of::<SyntaxBlockValue>(), size_of::<Arc<()>>());
        let backend = Arc::new(NativeCodeSyntaxOperations::default());
        let weak = Arc::downgrade(&backend);
        let syntax = backend.parse_body("return 42;").unwrap();
        let value = SyntaxBlockValue::new(etas_types::TypeId(7), syntax, backend.clone());
        let copy = value.clone();
        assert!(Arc::ptr_eq(&value.0, &copy.0));
        drop(copy);
        drop(backend);
        assert!(weak.upgrade().is_some());
        assert_eq!(
            value.format_controlled(64, &Continue).unwrap(),
            "return 42;"
        );
        assert!(matches!(
            value.format_controlled(4, &Continue),
            Err(SyntaxOperationError::ResourceLimit {
                resource: etas_code::syntax::SyntaxResource::FormattedBytes,
                limit: 4,
            })
        ));
        drop(value);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn syntax_values_cannot_be_coerced_to_host_values_or_live_checkpoint_handles() {
        let backend = Arc::new(NativeCodeSyntaxOperations::default());
        let syntax = backend.parse_body("return 42;").unwrap();
        let value = crate::value::InterpValue::SyntaxBlock(SyntaxBlockValue::new(
            etas_types::TypeId(7),
            syntax,
            backend,
        ));
        assert!(
            crate::eval::interp_to_host_value(&value)
                .unwrap_err()
                .contains("not host-encodable")
        );
        assert!(
            crate::orchestration::ValueSnapshot::capture(&value)
                .unwrap_err()
                .contains("locked definition environment")
        );
        let rendered = crate::api::codec::value_json(&value);
        assert!(crate::api::codec::value_from_json(&rendered).is_err());
    }
}
