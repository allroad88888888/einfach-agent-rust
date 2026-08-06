//! Live image resources injected into one session actor without entering durable state.

use std::sync::Arc;

use agent_runtime::{ImageResolver, RunnerCtx};

pub(crate) struct SessionImageRuntime {
    resolver: Arc<dyn ImageResolver>,
    upload_base_url: String,
}

impl SessionImageRuntime {
    pub(crate) fn new(resolver: Arc<dyn ImageResolver>, upload_base_url: String) -> Self {
        Self {
            resolver,
            upload_base_url,
        }
    }

    pub(super) fn inject(self, ctx: RunnerCtx) -> RunnerCtx {
        ctx.with_image_upload_base_url(self.upload_base_url)
            .with_image_resolver(self.resolver)
    }
}
