//! Project checked source Map storage into the host wire map. Hashing and map
//! operations remain in Etas; this adapter only observes live insertion slots.
use etas_host::value::projection::{HostValueProjection, HostValueVisitor};
use etas_host::{HostError, HostErrorCode};

use crate::value::InterpValue;

use super::projection::BorrowedValue;

pub(super) struct SourceMap<'a>(pub &'a InterpValue);

impl HostValueProjection for SourceMap<'_> {
    fn project<V: HostValueVisitor>(&self, visitor: V) -> Result<V::Output, HostError> {
        let entries = crate::value::source_map::entries(self.0)
            .map_err(|message| HostError::new(HostErrorCode::SchemaMismatch, message))?;
        visitor.map(
            entries
                .into_iter()
                .map(|(k, v)| (BorrowedValue(k), BorrowedValue(v))),
        )
    }
}
