//! Request-local conversion of attachment handles into visual-provider references.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_core::{ContentBlock, Message, PrefixImage, RequestIntent, SystemChunk, ToolSpec};
use agent_providers::{Ingredients, Provider};
use agent_transport::ImageUpload;

use crate::execution_binding::ExecutionBinding;
use crate::image_preparation_failure::ImagePreparationFailure;
use crate::image_resolver::{ImageResolver, ResolvedImageLease};

const ATTACHMENT_PREFIX: &str = "attachment://";

pub(crate) struct OwnedIngredients {
    pub(crate) system: Vec<SystemChunk>,
    pub(crate) messages: Vec<Message>,
    pub(crate) tools: Vec<ToolSpec>,
    pub(crate) late_tools: Vec<ToolSpec>,
    pub(crate) late_system: Vec<SystemChunk>,
    pub(crate) prev_prefix: Option<PrefixImage>,
}

pub(crate) enum ProviderRequest {
    Encoded(Vec<u8>),
    Deferred {
        ingredients: OwnedIngredients,
        resolver: Arc<dyn ImageResolver>,
    },
}

pub(crate) struct PreparedProviderRequest {
    body: Vec<u8>,
    _leases: Vec<Box<dyn ResolvedImageLease>>,
}

impl ProviderRequest {
    pub(crate) fn new(
        binding: &ExecutionBinding,
        encoded: Vec<u8>,
        ingredients: OwnedIngredients,
        resolver: Option<Arc<dyn ImageResolver>>,
    ) -> Result<Self, ImagePreparationFailure> {
        if !binding.provider.supports_images() || !has_attachment_images(&ingredients.messages) {
            return Ok(Self::Encoded(encoded));
        }
        let resolver = resolver.ok_or(ImagePreparationFailure::AttachmentUnavailable)?;
        Ok(Self::Deferred {
            ingredients,
            resolver,
        })
    }

    pub(crate) fn materializes_images(&self) -> bool {
        matches!(self, Self::Deferred { .. })
    }

    pub(crate) fn prepare(
        self,
        binding: &ExecutionBinding,
        cancel: &AtomicBool,
    ) -> Result<PreparedProviderRequest, ImagePreparationFailure> {
        match self {
            Self::Encoded(body) => Ok(PreparedProviderRequest {
                body,
                _leases: Vec::new(),
            }),
            Self::Deferred {
                mut ingredients,
                resolver,
            } => prepare_images(binding, &mut ingredients, resolver, cancel),
        }
    }
}

impl PreparedProviderRequest {
    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }
}

fn prepare_images(
    binding: &ExecutionBinding,
    ingredients: &mut OwnedIngredients,
    resolver: Arc<dyn ImageResolver>,
    cancel: &AtomicBool,
) -> Result<PreparedProviderRequest, ImagePreparationFailure> {
    ensure_not_cancelled(cancel)?;
    let handles = attachment_handles(&ingredients.messages);
    let mut leases = Vec::with_capacity(handles.len());
    for handle in &handles {
        ensure_not_cancelled(cancel)?;
        let lease = resolver.lease(handle).map_err(|error| match error {
            crate::ImageResolveError::Unknown => ImagePreparationFailure::AttachmentNotFound,
            crate::ImageResolveError::Unavailable => ImagePreparationFailure::AttachmentUnavailable,
        })?;
        if lease.bytes().is_empty() || !lease.mime().starts_with("image/") {
            return Err(ImagePreparationFailure::ImageUnsupported);
        }
        leases.push(lease);
    }

    let mut references = BTreeMap::new();
    for (handle, lease) in handles.iter().zip(&leases) {
        ensure_not_cancelled(cancel)?;
        let reference = binding
            .client
            .upload_image(
                &binding.image_upload_base_url,
                &binding.api_key,
                ImageUpload {
                    file_name: lease.name().unwrap_or("image"),
                    mime_type: lease.mime(),
                    bytes: lease.bytes(),
                },
            )
            .map_err(|error| ImagePreparationFailure::from_upload(&error))?;
        references.insert(handle.clone(), Arc::<str>::from(reference));
    }
    ensure_not_cancelled(cancel)?;
    replace_references(&mut ingredients.messages, &references, &leases, &handles);
    let body = encode(binding.provider.as_ref(), binding, ingredients);
    Ok(PreparedProviderRequest {
        body,
        _leases: leases,
    })
}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<(), ImagePreparationFailure> {
    if cancel.load(Ordering::Relaxed) {
        Err(ImagePreparationFailure::Cancelled)
    } else {
        Ok(())
    }
}

fn encode(
    provider: &dyn Provider,
    binding: &ExecutionBinding,
    owned: &OwnedIngredients,
) -> Vec<u8> {
    provider
        .encode(&Ingredients {
            system: &owned.system,
            messages: &owned.messages,
            tools: &owned.tools,
            late_tools: &owned.late_tools,
            late_system: &owned.late_system,
            config: &binding.session_config,
            intent: RequestIntent::Free,
            prev_prefix: owned.prev_prefix.as_ref(),
        })
        .body
}

fn has_attachment_images(messages: &[Message]) -> bool {
    messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Image { reference, .. } if reference.starts_with(ATTACHMENT_PREFIX))
        })
    })
}

fn attachment_handles(messages: &[Message]) -> Vec<String> {
    let mut handles = Vec::new();
    for message in messages {
        for block in &message.blocks {
            let ContentBlock::Image { reference, .. } = block else {
                continue;
            };
            if let Some(handle) = reference.strip_prefix(ATTACHMENT_PREFIX)
                && !handles.iter().any(|known| known == handle)
            {
                handles.push(handle.to_owned());
            }
        }
    }
    handles
}

fn replace_references(
    messages: &mut [Message],
    references: &BTreeMap<String, Arc<str>>,
    leases: &[Box<dyn ResolvedImageLease>],
    handles: &[String],
) {
    for message in messages {
        for block in &mut message.blocks {
            let ContentBlock::Image {
                reference,
                mime,
                name,
            } = block
            else {
                continue;
            };
            let Some(handle) = reference.strip_prefix(ATTACHMENT_PREFIX) else {
                continue;
            };
            let Some(index) = handles.iter().position(|known| known == handle) else {
                continue;
            };
            *reference = Arc::clone(&references[handle]);
            *mime = Arc::from(leases[index].mime());
            *name = leases[index].name().map(Arc::from);
        }
    }
}

#[cfg(test)]
#[path = "image_materialization_tests.rs"]
mod tests;
