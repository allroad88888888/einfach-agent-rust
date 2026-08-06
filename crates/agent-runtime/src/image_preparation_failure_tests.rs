use agent_core::ErrorClass;
use agent_transport::UploadError;

use super::ImagePreparationFailure;

#[test]
fn upload_http_statuses_have_stable_retry_semantics() {
    use ImagePreparationFailure::{ImageUnsupported, VisionRejected, VisionUploadFailed};

    let cases = [
        (UploadError::Unauthorized, VisionRejected),
        (rejected(400), ImageUnsupported),
        (rejected(401), VisionRejected),
        (rejected(403), VisionRejected),
        (rejected(404), VisionRejected),
        (rejected(413), ImageUnsupported),
        (rejected(415), ImageUnsupported),
        (rejected(422), ImageUnsupported),
        (rejected(408), VisionUploadFailed),
        (rejected(429), VisionUploadFailed),
        (rejected(500), VisionUploadFailed),
        (rejected(503), VisionUploadFailed),
    ];

    for (upload, expected) in cases {
        let actual = ImagePreparationFailure::from_upload(&upload);
        assert_eq!(actual, expected, "unexpected classification for {upload:?}");
        assert_eq!(
            actual.error_class() == ErrorClass::Retryable,
            expected == VisionUploadFailed,
            "retryability disagrees for {upload:?}"
        );
    }
}

#[test]
fn transport_and_response_failures_remain_retryable() {
    for upload in [
        UploadError::Network {
            message: "redacted network failure".to_owned(),
        },
        UploadError::InvalidResponse {
            message: "redacted invalid response".to_owned(),
        },
    ] {
        let failure = ImagePreparationFailure::from_upload(&upload);
        assert_eq!(failure, ImagePreparationFailure::VisionUploadFailed);
        assert_eq!(failure.error_class(), ErrorClass::Retryable);
        assert!(failure.vision_failure().retryable());
    }
}

fn rejected(status: u16) -> UploadError {
    UploadError::ProviderRejected { status }
}
