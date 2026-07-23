mod abi;
mod call;
mod fast_path;

pub use abi::{AdapterError, PureAbiProjector};
pub use call::execute_pure_intrinsic;

#[cfg(test)]
mod tests;
