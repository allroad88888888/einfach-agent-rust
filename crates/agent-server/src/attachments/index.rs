use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use crate::SessionId;

use super::error::{LeaseError, RegisterError};
use super::handle::ImageHandle;
use super::record::ImageData;
use super::store::AttachmentVaultConfig;

const MAX_CLOSED_SESSIONS: usize = 256;
const MAX_UNAVAILABLE_HANDLES: usize = 4096;

#[derive(Default)]
pub(crate) struct Inner {
    records: HashMap<ImageHandle, Record>,
    sessions: HashMap<SessionId, Usage>,
    closed_sessions: HashSet<SessionId>,
    closed_order: VecDeque<SessionId>,
    unavailable_order: VecDeque<ImageHandle>,
    global: Usage,
}

enum Record {
    Available(AvailableRecord),
    Unavailable { owner: SessionId },
}

struct AvailableRecord {
    owner: SessionId,
    image: Arc<ImageData>,
    expires_at: Instant,
    leases: usize,
    unavailable_pending: bool,
}

#[derive(Default)]
struct Usage {
    images: usize,
    bytes: usize,
}

impl Inner {
    pub(crate) fn check_quota(
        &self,
        owner: &SessionId,
        bytes: usize,
        config: &AttachmentVaultConfig,
    ) -> Result<(), RegisterError> {
        if self.closed_sessions.contains(owner) {
            return Err(RegisterError::SessionClosed);
        }
        let empty = Usage::default();
        let session = self.sessions.get(owner).unwrap_or(&empty);
        if session.images >= config.max_session_images {
            return Err(RegisterError::SessionImageLimit {
                limit: config.max_session_images,
            });
        }
        if session.bytes.saturating_add(bytes) > config.max_session_bytes {
            return Err(RegisterError::SessionByteLimit {
                limit_bytes: config.max_session_bytes,
            });
        }
        if self.global.images >= config.max_global_images {
            return Err(RegisterError::GlobalImageLimit {
                limit: config.max_global_images,
            });
        }
        if self.global.bytes.saturating_add(bytes) > config.max_global_bytes {
            return Err(RegisterError::GlobalByteLimit {
                limit_bytes: config.max_global_bytes,
            });
        }
        Ok(())
    }
    pub(crate) fn insert(
        &mut self,
        handle: ImageHandle,
        owner: SessionId,
        image: Arc<ImageData>,
        expires_at: Instant,
    ) {
        self.add_usage(&owner, image.byte_len());
        self.records.insert(
            handle,
            Record::Available(AvailableRecord {
                owner,
                image,
                expires_at,
                leases: 0,
                unavailable_pending: false,
            }),
        );
    }
    pub(crate) fn lease(
        &mut self,
        owner: &SessionId,
        handle: &ImageHandle,
    ) -> Result<Arc<ImageData>, LeaseError> {
        match self.records.get_mut(handle) {
            None => Err(LeaseError::Unknown),
            Some(Record::Unavailable { owner: known_owner }) if known_owner == owner => {
                Err(LeaseError::Unavailable)
            }
            Some(Record::Unavailable { .. }) => Err(LeaseError::Unknown),
            Some(Record::Available(record)) if &record.owner != owner => Err(LeaseError::Unknown),
            Some(Record::Available(record)) if record.unavailable_pending => {
                Err(LeaseError::Unavailable)
            }
            Some(Record::Available(record)) => {
                record.leases += 1;
                Ok(Arc::clone(&record.image))
            }
        }
    }
    pub(crate) fn lease_at(
        &mut self,
        owner: &SessionId,
        handle: &ImageHandle,
        now: Instant,
    ) -> Result<Arc<ImageData>, LeaseError> {
        match self.records.get(handle) {
            None => return Err(LeaseError::Unknown),
            Some(Record::Unavailable { owner: known_owner }) if known_owner == owner => {
                return Err(LeaseError::Unavailable);
            }
            Some(Record::Unavailable { .. }) => return Err(LeaseError::Unknown),
            Some(Record::Available(record)) if &record.owner != owner => {
                return Err(LeaseError::Unknown);
            }
            Some(Record::Available(_)) => {}
        }
        self.expire_one(handle, now);
        self.lease(owner, handle)
    }
    pub(crate) fn expire_one(&mut self, handle: &ImageHandle, now: Instant) {
        let should_expire = matches!(self.records.get(handle), Some(Record::Available(record)) if record.expires_at <= now && record.leases == 0);
        let pending = matches!(self.records.get(handle), Some(Record::Available(record)) if record.expires_at <= now && record.leases > 0);
        if should_expire {
            self.make_unavailable(handle);
        }
        if pending {
            if let Some(Record::Available(record)) = self.records.get_mut(handle) {
                record.unavailable_pending = true;
            }
        }
    }
    pub(crate) fn sweep(&mut self, now: Instant) -> usize {
        let handles: Vec<_> = self.records.keys().cloned().collect();
        let mut reclaimed = 0;
        for handle in handles {
            let was_available = matches!(self.records.get(&handle), Some(Record::Available(_)));
            self.expire_one(&handle, now);
            if was_available
                && matches!(self.records.get(&handle), Some(Record::Unavailable { .. }))
            {
                reclaimed += 1;
            }
        }
        reclaimed
    }
    pub(crate) fn unavailable_session(&mut self, owner: &SessionId) -> usize {
        if self.closed_sessions.insert(owner.clone()) {
            self.closed_order.push_back(owner.clone());
            self.prune_closed_sessions();
        }
        let handles: Vec<_> = self
            .records
            .iter()
            .filter_map(|(handle, record)| match record {
                Record::Available(record) if &record.owner == owner => Some(handle.clone()),
                _ => None,
            })
            .collect();
        for handle in &handles {
            self.make_unavailable(handle);
        }
        handles.len()
    }
    pub(crate) fn begin_session(&mut self, owner: &SessionId) {
        self.closed_sessions.remove(owner);
        self.closed_order.retain(|id| id != owner);
    }

    pub(crate) fn seed_unavailable(&mut self, owner: &SessionId, handle: ImageHandle) {
        if self.records.contains_key(&handle) {
            return;
        }
        self.records.insert(
            handle.clone(),
            Record::Unavailable {
                owner: owner.clone(),
            },
        );
        self.unavailable_order.push_back(handle);
        self.prune_unavailable_handles();
    }

    pub(crate) fn evict(
        &mut self,
        owner: &SessionId,
        handle: &ImageHandle,
    ) -> Result<(), LeaseError> {
        match self.records.get(handle) {
            None => return Err(LeaseError::Unknown),
            Some(Record::Unavailable { owner: known_owner }) if known_owner == owner => {
                return Err(LeaseError::Unavailable);
            }
            Some(Record::Unavailable { .. }) => return Err(LeaseError::Unknown),
            Some(Record::Available(record)) if &record.owner != owner => {
                return Err(LeaseError::Unknown);
            }
            Some(Record::Available(_)) => {}
        }
        self.make_unavailable(handle);
        Ok(())
    }

    pub(crate) fn release(&mut self, handle: &ImageHandle) {
        let reclaim = match self.records.get_mut(handle) {
            Some(Record::Available(record)) => {
                record.leases = record.leases.saturating_sub(1);
                record.leases == 0 && record.unavailable_pending
            }
            _ => false,
        };
        if reclaim {
            self.make_unavailable(handle);
        }
    }

    fn make_unavailable(&mut self, handle: &ImageHandle) {
        if let Some(Record::Available(record)) = self.records.get_mut(handle)
            && record.leases > 0
        {
            record.unavailable_pending = true;
            return;
        }
        let Some(record) = self.records.remove(handle) else {
            return;
        };
        match record {
            Record::Available(record) => {
                self.remove_usage(&record.owner, record.image.byte_len());
                self.records.insert(
                    handle.clone(),
                    Record::Unavailable {
                        owner: record.owner,
                    },
                );
                self.unavailable_order.push_back(handle.clone());
                self.prune_unavailable_handles();
            }
            unavailable => {
                self.records.insert(handle.clone(), unavailable);
            }
        }
    }

    fn add_usage(&mut self, owner: &SessionId, bytes: usize) {
        let usage = self.sessions.entry(owner.clone()).or_default();
        usage.images += 1;
        usage.bytes += bytes;
        self.global.images += 1;
        self.global.bytes += bytes;
    }

    fn remove_usage(&mut self, owner: &SessionId, bytes: usize) {
        if let Some(usage) = self.sessions.get_mut(owner) {
            usage.images -= 1;
            usage.bytes -= bytes;
            if usage.images == 0 {
                self.sessions.remove(owner);
            }
        }
        self.global.images -= 1;
        self.global.bytes -= bytes;
    }

    fn prune_closed_sessions(&mut self) {
        while self.closed_order.len() > MAX_CLOSED_SESSIONS {
            if let Some(owner) = self.closed_order.pop_front() {
                self.closed_sessions.remove(&owner);
            }
        }
    }

    fn prune_unavailable_handles(&mut self) {
        while self.unavailable_order.len() > MAX_UNAVAILABLE_HANDLES {
            let Some(handle) = self.unavailable_order.pop_front() else {
                break;
            };
            if matches!(self.records.get(&handle), Some(Record::Unavailable { .. })) {
                self.records.remove(&handle);
            }
        }
    }
}
