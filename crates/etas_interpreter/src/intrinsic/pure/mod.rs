mod abi;
mod call;
mod count;
mod fast_path;
mod text;

pub use abi::{AdapterError, PureAbiProjector};
pub use call::execute_pure_intrinsic;

#[cfg(test)]
mod adt_tests;
#[cfg(test)]
mod bytes_tests;
#[cfg(test)]
mod count_tests;
#[cfg(test)]
mod crypto_tests;
#[cfg(test)]
mod fast_path_validation_tests;
#[cfg(test)]
mod join_tests;
#[cfg(test)]
mod record_input_tests;
#[cfg(test)]
mod record_tests;
#[cfg(test)]
mod rope_case_tests;
#[cfg(test)]
mod rope_query_tests;
#[cfg(test)]
mod rope_transform_tests;
#[cfg(test)]
mod sequence_input_tests;
#[cfg(test)]
mod shared_field_input_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod text_tests;
#[cfg(test)]
mod text_transform_tests;
