//! Request-time access to host-owned image bytes.

use std::sync::Arc;

use crate::ctx::RunnerCtx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageResolveError {
    Unknown,
    Unavailable,
}

/// A live borrow of one image. Dropping it releases the host's retention lease.
pub trait ResolvedImageLease: Send {
    fn mime(&self) -> &str;
    fn name(&self) -> Option<&str>;
    fn bytes(&self) -> &[u8];
}

/// Host seam for resolving a durable `attachment://...` handle without exposing bytes to core.
pub trait ImageResolver: Send + Sync {
    fn lease(&self, handle: &str) -> Result<Box<dyn ResolvedImageLease>, ImageResolveError>;
}

impl RunnerCtx {
    pub fn with_image_resolver(mut self, resolver: Arc<dyn ImageResolver>) -> Self {
        self.image_resolver = Some(resolver);
        self
    }
}
