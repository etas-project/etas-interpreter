mod error;
pub(in crate::intrinsic::pure) mod input;
pub(in crate::intrinsic::pure) mod output;
mod shape;

pub use error::AdapterError;
pub(super) use shape::AbiShape;
pub use shape::PureAbiProjector;

#[derive(Clone, Copy)]
enum SequenceKind {
    Array,
    List,
    Slice,
    Set,
}
