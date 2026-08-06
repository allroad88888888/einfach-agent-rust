use std::sync::Arc;

pub(crate) struct ImageData {
    mime: Arc<str>,
    name: Option<Arc<str>>,
    bytes: Arc<[u8]>,
}

impl ImageData {
    pub(crate) fn new(mime: &str, name: Option<&str>, bytes: &[u8]) -> Self {
        Self {
            mime: Arc::from(mime),
            name: name.map(Arc::from),
            bytes: Arc::from(bytes),
        }
    }

    pub(crate) fn mime(&self) -> &str {
        &self.mime
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}
