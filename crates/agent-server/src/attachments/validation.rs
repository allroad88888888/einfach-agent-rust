use super::error::RegisterError;
use super::mime;
use super::store::ImageRegistration;

pub(crate) fn validate(image: &ImageRegistration<'_>) -> Result<(), RegisterError> {
    if image.bytes.is_empty() {
        return Err(RegisterError::EmptyImage);
    }
    mime::validate(image.mime, image.bytes)?;
    if image.name.is_some_and(invalid_name) {
        return Err(RegisterError::InvalidName);
    }
    Ok(())
}

fn invalid_name(name: &str) -> bool {
    name.is_empty()
        || name.len() > 255
        || name.chars().any(char::is_control)
        || name.contains('/')
        || name.contains('\\')
        || matches!(name, "." | "..")
        || name.as_bytes().get(1) == Some(&b':')
}

#[cfg(test)]
mod tests {
    use super::invalid_name;

    #[test]
    fn rejects_path_shaped_names_without_rejecting_unicode_basenames() {
        for name in [
            "/private/canary.png",
            "../canary.png",
            r"C:\private\canary.png",
            r"\\server\share\canary.png",
            "C:canary.png",
            ".",
            "..",
        ] {
            assert!(invalid_name(name), "accepted path-shaped name: {name:?}");
        }
        for name in ["photo.png", "截图 2026-08-06.png", "résumé.jpeg"] {
            assert!(!invalid_name(name), "rejected basename: {name:?}");
        }
    }
}
