use etas_types::TypeId;

/// A scoped instance reference. State is owned by the invocation's scheduler,
/// so copying this value cannot copy state or retain an update turn.
#[derive(Clone, Copy, Debug)]
pub struct AgentReference {
    pub(crate) run: u64,
    pub(crate) id: u64,
    pub(crate) ty: TypeId,
    /// Checked interface presenting this reference; never serialized or dereferenced.
    pub(crate) program: usize,
}

// Type projection changes only the presenting interface, not instance identity.
impl PartialEq for AgentReference {
    fn eq(&self, other: &Self) -> bool {
        self.run == other.run && self.id == other.id
    }
}
impl Eq for AgentReference {}

#[derive(Clone)]
pub(crate) struct MemberToolSnapshot {
    pub(crate) instance: AgentReference,
    pub(crate) request: crate::driver::scheduler::TaskIdentity,
    pub(crate) state: super::InterpValue,
    pub(crate) calls: std::collections::BTreeMap<String, etas_hir::SymbolId>,
}
