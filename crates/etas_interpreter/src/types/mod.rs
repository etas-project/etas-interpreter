mod catalog;
mod identity;
mod layouts;
mod resolve;

pub(crate) use catalog::CheckedTypeCatalog;
pub use identity::CheckedTypeIdentity;
pub(crate) use identity::{DeclarationIdentity, TypeHead};
pub use layouts::CheckedVariantLayout;

#[cfg(test)]
mod tests;
