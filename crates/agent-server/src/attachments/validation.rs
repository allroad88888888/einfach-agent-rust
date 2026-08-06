use super::error::RegisterError;
use super::store::ImageRegistration;

pub(crate) fn validate(image: &ImageRegistration<'_>) -> Result<(), RegisterError> {
    if image.bytes.is_empty() {
        return Err(RegisterError::EmptyImage);
    }
    let subtype = image
        .mime
        .strip_prefix("image/")
        .filter(|part| !part.is_empty());
    if !subtype.is_some_and(|part| {
        part.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
    }) {
        return Err(RegisterError::InvalidMime);
    }
    if image.name.is_some_and(|name| {
        name.is_empty() || name.len() > 255 || name.chars().any(char::is_control)
    }) {
        return Err(RegisterError::InvalidName);
    }
    Ok(())
}
