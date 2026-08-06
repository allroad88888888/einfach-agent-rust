use super::error::RegisterError;
use super::mime;
use super::store::ImageRegistration;

pub(crate) fn validate(image: &ImageRegistration<'_>) -> Result<(), RegisterError> {
    if image.bytes.is_empty() {
        return Err(RegisterError::EmptyImage);
    }
    mime::validate(image.mime, image.bytes)?;
    if image.name.is_some_and(|name| {
        name.is_empty() || name.len() > 255 || name.chars().any(char::is_control)
    }) {
        return Err(RegisterError::InvalidName);
    }
    Ok(())
}
