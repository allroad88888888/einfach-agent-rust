use std::time::Instant;

use crate::SessionId;

use super::{AttachmentVault, AttachmentVaultConfig, ImageHandle, ImageRegistration, LeaseError};

const OLD_JPEG: &[u8] = b"\xff\xd8\xffold";
const NEW_JPEG: &[u8] = b"\xff\xd8\xffnew";

#[test]
fn closed_session_id_can_register_a_fresh_image_after_reopen() {
    let vault = AttachmentVault::new(AttachmentVaultConfig::default());
    let session = SessionId::from("same-chat-id");
    let old = register(&vault, &session, OLD_JPEG);

    vault.close_session(&session);
    vault.begin_session(&session);
    let new = register(&vault, &session, NEW_JPEG);

    assert_ne!(old, new);
    assert!(matches!(
        vault.lease(&session, &old, Instant::now()),
        Err(LeaseError::Unavailable)
    ));
    assert_eq!(
        vault.lease(&session, &new, Instant::now()).unwrap().bytes(),
        NEW_JPEG
    );
}

#[test]
fn recovered_handle_is_unavailable_and_never_reused_for_new_bytes() {
    let vault = AttachmentVault::new(AttachmentVaultConfig::default());
    let session = SessionId::from("recovered-chat-id");
    let restored = ImageHandle::parse("img_1000000").unwrap();

    vault.begin_session(&session);
    vault.seed_unavailable(&session, [restored.clone()]);
    let new = register(&vault, &session, NEW_JPEG);

    assert!(matches!(
        vault.lease(&session, &restored, Instant::now()),
        Err(LeaseError::Unavailable)
    ));
    let number = new
        .as_str()
        .strip_prefix("img_")
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(number > 1_000_000, "恢复句柄后序号必须前进：{new}");
    assert_eq!(
        vault.lease(&session, &new, Instant::now()).unwrap().bytes(),
        NEW_JPEG
    );
}

fn register(vault: &AttachmentVault, session: &SessionId, bytes: &[u8]) -> ImageHandle {
    vault
        .register(
            session,
            ImageRegistration {
                mime: "image/jpeg",
                name: None,
                bytes,
            },
            Instant::now(),
        )
        .unwrap()
}
