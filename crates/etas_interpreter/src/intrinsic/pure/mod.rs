mod abi;
mod call;
mod fast_path;
mod text;

pub use abi::{AdapterError, PureAbiProjector};
pub use call::execute_pure_intrinsic;

#[cfg(test)]
mod adt_tests;
#[cfg(test)]
mod bytes_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod text_tests;
#[cfg(test)]
mod text_transform_tests;
