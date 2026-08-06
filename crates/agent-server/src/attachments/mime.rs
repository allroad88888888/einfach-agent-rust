use super::error::RegisterError;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG_SIGNATURE: &[u8] = b"\xff\xd8\xff";
const GIF_87A_SIGNATURE: &[u8] = b"GIF87a";
const GIF_89A_SIGNATURE: &[u8] = b"GIF89a";

/// Verifies that the declared, supported MIME type agrees with the file signature.
/// Full image decoding and structural validation remain the consumer's responsibility.
pub(crate) fn validate(mime: &str, bytes: &[u8]) -> Result<(), RegisterError> {
    let matches = match mime {
        "image/png" => bytes.starts_with(PNG_SIGNATURE),
        "image/jpeg" => bytes.starts_with(JPEG_SIGNATURE),
        "image/gif" => bytes.starts_with(GIF_87A_SIGNATURE) || bytes.starts_with(GIF_89A_SIGNATURE),
        "image/webp" => is_webp(bytes),
        // SVG is active XML content and has no unambiguous binary signature. Keep it
        // outside the attachment allowlist rather than treating arbitrary XML as an image.
        _ => false,
    };
    matches.then_some(()).ok_or(RegisterError::InvalidMime)
}

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_raster_signatures_match_their_declared_mime() {
        let samples: [(&str, &[u8]); 5] = [
            ("image/png", b"\x89PNG\r\n\x1a\nrest"),
            ("image/jpeg", b"\xff\xd8\xffrest"),
            ("image/gif", b"GIF87arest"),
            ("image/gif", b"GIF89arest"),
            ("image/webp", b"RIFF\x04\x00\x00\x00WEBPrest"),
        ];

        for (mime, bytes) in samples {
            assert_eq!(validate(mime, bytes), Ok(()), "rejected {mime}");
        }
    }

    #[test]
    fn a_valid_signature_cannot_be_spoofed_as_another_mime() {
        assert_eq!(
            validate("image/jpeg", b"\x89PNG\r\n\x1a\n"),
            Err(RegisterError::InvalidMime)
        );
        assert_eq!(
            validate("image/png", b"\xff\xd8\xff"),
            Err(RegisterError::InvalidMime)
        );
    }

    #[test]
    fn truncated_signatures_use_the_stable_invalid_mime_category() {
        let samples: [(&str, &[u8]); 4] = [
            ("image/png", b"\x89PNG\r\n\x1a"),
            ("image/jpeg", b"\xff\xd8"),
            ("image/gif", b"GIF89"),
            ("image/webp", b"RIFF\x00\x00\x00\x00WEB"),
        ];

        for (mime, bytes) in samples {
            assert_eq!(
                validate(mime, bytes),
                Err(RegisterError::InvalidMime),
                "accepted truncated {mime}"
            );
        }
    }

    #[test]
    fn svg_and_other_unlisted_image_types_are_rejected() {
        for (mime, bytes) in [
            (
                "image/svg+xml",
                b"<svg xmlns='http://www.w3.org/2000/svg'/>" as &[u8],
            ),
            ("image/bmp", b"BMrest" as &[u8]),
        ] {
            assert_eq!(validate(mime, bytes), Err(RegisterError::InvalidMime));
        }
    }
}
