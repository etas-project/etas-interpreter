mod error;
pub(in crate::intrinsic::pure) mod input;
pub(in crate::intrinsic::pure) mod output;
mod record;
mod result;
mod shape;
pub(super) mod text;
mod wrapper_walk;

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
pub(super) mod borrowed;
