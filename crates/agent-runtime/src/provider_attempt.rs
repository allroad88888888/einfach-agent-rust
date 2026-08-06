//! Process-local identity for one launched provider request.
//!
//! An epoch identifies a state-machine generation, not a transport attempt: a retry can launch
//! again in the same epoch while the abandoned IO thread is still returning late messages.

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PROVIDER_ATTEMPT: AtomicU64 = AtomicU64::new(1);

/// Unique identity shared by one [`crate::provider_call::ProviderCall`] and its IO messages.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProviderAttemptId(u64);

impl ProviderAttemptId {
    pub(crate) fn allocate() -> Self {
        let id = NEXT_PROVIDER_ATTEMPT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("provider attempt id space exhausted");
        Self(id)
    }
}
