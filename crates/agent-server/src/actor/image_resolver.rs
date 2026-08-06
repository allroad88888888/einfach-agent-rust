//! Session-scoped adapter from the attachment vault to runtime request materialization.

use std::sync::Arc;
use std::time::Instant;

use agent_runtime::{ImageResolveError, ImageResolver, ResolvedImageLease};

use crate::SessionId;
use crate::attachments::{AttachmentVault, ImageHandle, ImageLease, LeaseError};

pub(crate) fn session_image_resolver(
    vault: AttachmentVault,
    owner: SessionId,
) -> Arc<dyn ImageResolver> {
    Arc::new(SessionImageResolver { vault, owner })
}

struct SessionImageResolver {
    vault: AttachmentVault,
    owner: SessionId,
}

impl ImageResolver for SessionImageResolver {
    fn lease(&self, handle: &str) -> Result<Box<dyn ResolvedImageLease>, ImageResolveError> {
        let handle = ImageHandle::parse(handle).ok_or(ImageResolveError::Unknown)?;
        self.vault
            .lease(&self.owner, &handle, Instant::now())
            .map(|lease| Box::new(lease) as Box<dyn ResolvedImageLease>)
            .map_err(|error| match error {
                LeaseError::Unknown => ImageResolveError::Unknown,
                LeaseError::Unavailable => ImageResolveError::Unavailable,
            })
    }
}

impl ResolvedImageLease for ImageLease {
    fn mime(&self) -> &str {
        ImageLease::mime(self)
    }

    fn name(&self) -> Option<&str> {
        ImageLease::name(self)
    }

    fn bytes(&self) -> &[u8] {
        ImageLease::bytes(self)
    }
}
