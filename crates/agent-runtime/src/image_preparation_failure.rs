//! Stable, sanitized failures produced before a visual provider request can start.

use agent_core::vision::VisionFailure;
use agent_core::{AgentId, ErrorClass};
use agent_transport::UploadError;

use crate::ctx::RunnerCtx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImagePreparationFailure {
    Cancelled,
    AttachmentNotFound,
    AttachmentUnavailable,
    ImageUnsupported,
    VisionRejected,
    VisionUploadFailed,
}

impl ImagePreparationFailure {
    pub(crate) fn error_class(self) -> ErrorClass {
        match self {
            Self::Cancelled => ErrorClass::Unknown,
            Self::AttachmentNotFound | Self::AttachmentUnavailable | Self::ImageUnsupported => {
                ErrorClass::BadRequest
            }
            Self::VisionRejected => ErrorClass::BadRequest,
            Self::VisionUploadFailed => ErrorClass::Retryable,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "image preparation was cancelled",
            Self::AttachmentNotFound => "a selected image was not found in this session",
            Self::AttachmentUnavailable => "a selected image is no longer available",
            Self::ImageUnsupported => "a selected image format or payload is unsupported",
            Self::VisionRejected => "image upload was rejected by provider or policy",
            Self::VisionUploadFailed => "a selected image could not be prepared for vision",
        }
    }

    pub(crate) fn vision_failure(self) -> VisionFailure {
        match self {
            Self::Cancelled => VisionFailure::cancelled(),
            Self::AttachmentNotFound => VisionFailure::attachment_not_found(),
            Self::AttachmentUnavailable => VisionFailure::attachment_unavailable(),
            Self::ImageUnsupported => VisionFailure::image_unsupported(),
            Self::VisionRejected => VisionFailure::rejected(),
            Self::VisionUploadFailed => VisionFailure::upload_failed(),
        }
    }

    pub(crate) fn from_upload(error: &UploadError) -> Self {
        match error {
            UploadError::TooLarge { .. }
            | UploadError::ProviderRejected {
                status: 400 | 413 | 415 | 422,
            } => Self::ImageUnsupported,
            UploadError::Unauthorized
            | UploadError::ProviderRejected {
                status: 401 | 403 | 404,
            } => Self::VisionRejected,
            UploadError::ProviderRejected { status: 408 | 429 } => Self::VisionUploadFailed,
            UploadError::ProviderRejected { status } if (400..500).contains(status) => {
                Self::VisionRejected
            }
            UploadError::ProviderRejected { .. }
            | UploadError::Network { .. }
            | UploadError::InvalidResponse { .. } => Self::VisionUploadFailed,
        }
    }
}

#[cfg(test)]
#[path = "image_preparation_failure_tests.rs"]
mod tests;

impl RunnerCtx {
    pub(crate) fn record_image_preparation_failure(
        &mut self,
        agent: AgentId,
        failure: ImagePreparationFailure,
    ) {
        self.image_preparation_failures.insert(agent, failure);
    }

    pub(crate) fn clear_image_preparation_failure(&mut self, agent: &AgentId) {
        self.image_preparation_failures.remove(agent);
    }

    pub(crate) fn take_image_preparation_failure(
        &mut self,
        agent: &AgentId,
    ) -> Option<ImagePreparationFailure> {
        self.image_preparation_failures.remove(agent)
    }

    pub(crate) fn clear_image_preparation_failures(&mut self) {
        self.image_preparation_failures.clear();
    }
}
