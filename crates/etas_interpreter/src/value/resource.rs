#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemorySelectionKind {
    Select,
    Query,
    Scan,
    RelatedTo,
}
