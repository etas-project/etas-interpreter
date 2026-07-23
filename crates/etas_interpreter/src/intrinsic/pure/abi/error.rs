use etas_builtin::BuiltinError;
use etas_types::TypeId;

#[derive(Clone, Debug, PartialEq)]
pub enum AdapterError {
    Builtin(BuiltinError),
    Arity { expected: usize, actual: usize },
    MissingType(TypeId),
    TypeMismatch { expected: TypeId, actual: String },
    NominalIdentity { expected: TypeId, actual: TypeId },
    UnsupportedValue(String),
}
