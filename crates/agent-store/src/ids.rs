/// Unique identifier for an atom in the store.
#[derive(Clone, Copy, Hash, Eq, PartialEq, Debug)]
pub struct AtomId(pub(crate) u64);

impl AtomId {
    /// Create an AtomId from a raw u64. For testing only.
    pub fn from_raw(id: u64) -> Self {
        AtomId(id)
    }
}
