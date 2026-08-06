use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::SessionId;

use super::{
    AttachmentVault, AttachmentVaultConfig, ImageHandle, ImageRegistration, LeaseError,
    RegisterError,
};

fn vault() -> AttachmentVault {
    AttachmentVault::new(AttachmentVaultConfig {
        max_image_bytes: 16,
        max_session_images: 4,
        max_session_bytes: 16,
        max_global_images: 8,
        max_global_bytes: 32,
        ttl: Duration::from_secs(1),
    })
}

fn image(bytes: &'static [u8]) -> ImageRegistration<'static> {
    ImageRegistration {
        mime: "image/png",
        name: Some("photo.png"),
        bytes,
    }
}

fn session(name: &str) -> SessionId {
    SessionId::from(name)
}

#[test]
fn register_copies_validated_image_and_returns_img_handle() {
    let vault = vault();
    let owner = session("one");
    let now = Instant::now();
    let handle = vault.register(&owner, image(b"image"), now).unwrap();

    assert!(handle.as_str().starts_with("img_"));
    let lease = vault.lease(&owner, &handle, now).unwrap();
    assert_eq!(lease.mime(), "image/png");
    assert_eq!(lease.name(), Some("photo.png"));
    assert_eq!(lease.bytes(), b"image");
}

#[test]
fn exposed_handles_parse_strictly_but_do_not_authorize_access() {
    assert_eq!(ImageHandle::parse("img_42").unwrap().as_str(), "img_42");
    for invalid in ["img_", "img_-1", "img_42/next", "attachment://img_42"] {
        assert!(
            ImageHandle::parse(invalid).is_none(),
            "accepted {invalid:?}"
        );
    }

    let vault = vault();
    let owner = session("owner");
    let other = session("other");
    let now = Instant::now();
    let handle = vault.register(&owner, image(b"one"), now).unwrap();
    let parsed = ImageHandle::parse(handle.as_str()).unwrap();
    assert!(matches!(
        vault.lease(&other, &parsed, now),
        Err(LeaseError::Unknown)
    ));
}

#[test]
fn invalid_input_has_no_payload_in_error() {
    let vault = vault();
    let owner = session("one");
    let error = vault
        .register(
            &owner,
            ImageRegistration {
                mime: "text/plain",
                name: Some("/private/provider-ref.png"),
                bytes: b"secret-image-bytes",
            },
            Instant::now(),
        )
        .unwrap_err();

    let rendered = format!("{error:?} {error}");
    assert_eq!(error, RegisterError::InvalidMime);
    assert!(!rendered.contains("secret-image-bytes"));
    assert!(!rendered.contains("/private/provider-ref.png"));
}

#[test]
fn quotas_reject_before_mutating_accounting() {
    let vault = AttachmentVault::new(AttachmentVaultConfig {
        max_image_bytes: 3,
        max_session_images: 1,
        max_session_bytes: 3,
        max_global_images: 1,
        max_global_bytes: 3,
        ttl: Duration::from_secs(1),
    });
    let one = session("one");
    let two = session("two");
    let now = Instant::now();

    assert_eq!(
        vault.register(&one, image(b"toolarge"), now),
        Err(RegisterError::ImageByteLimit { limit_bytes: 3 })
    );
    let first = vault.register(&one, image(b"one"), now).unwrap();
    assert_eq!(
        vault.register(&one, image(b"two"), now),
        Err(RegisterError::SessionImageLimit { limit: 1 })
    );
    assert_eq!(
        vault.register(&two, image(b"two"), now),
        Err(RegisterError::GlobalImageLimit { limit: 1 })
    );
    vault.evict(&one, &first).unwrap();
    assert!(vault.register(&two, image(b"two"), now).is_ok());
}

#[test]
fn byte_quotas_are_independent_of_image_count_quotas() {
    let vault = AttachmentVault::new(AttachmentVaultConfig {
        max_image_bytes: 8,
        max_session_images: 4,
        max_session_bytes: 3,
        max_global_images: 8,
        max_global_bytes: 4,
        ttl: Duration::from_secs(1),
    });
    let one = session("one");
    let two = session("two");
    let now = Instant::now();

    vault.register(&one, image(b"one"), now).unwrap();
    assert_eq!(
        vault.register(&one, image(b"x"), now),
        Err(RegisterError::SessionByteLimit { limit_bytes: 3 })
    );
    assert_eq!(
        vault.register(&two, image(b"xx"), now),
        Err(RegisterError::GlobalByteLimit { limit_bytes: 4 })
    );
}

#[test]
fn ownership_hides_handles_belonging_to_other_sessions() {
    let vault = vault();
    let owner = session("one");
    let other = session("two");
    let now = Instant::now();
    let handle = vault.register(&owner, image(b"one"), now).unwrap();

    assert!(matches!(
        vault.lease(&other, &handle, now),
        Err(LeaseError::Unknown)
    ));
    assert_eq!(vault.evict(&other, &handle), Err(LeaseError::Unknown));
    vault.close_session(&owner);
    assert!(matches!(
        vault.lease(&owner, &handle, now),
        Err(LeaseError::Unavailable)
    ));
}

#[test]
fn expiry_waits_for_lease_then_becomes_unavailable() {
    let vault = vault();
    let owner = session("one");
    let start = Instant::now();
    let handle = vault.register(&owner, image(b"one"), start).unwrap();
    let lease = vault.lease(&owner, &handle, start).unwrap();
    let expired = start + Duration::from_secs(2);

    assert_eq!(vault.sweep(expired), 0);
    assert_eq!(lease.bytes(), b"one");
    assert!(matches!(
        vault.lease(&owner, &handle, expired),
        Err(LeaseError::Unavailable)
    ));
    drop(lease);
    assert!(matches!(
        vault.lease(&owner, &handle, expired),
        Err(LeaseError::Unavailable)
    ));
}

#[test]
fn close_and_evict_revoke_new_reads_without_breaking_existing_lease() {
    let vault = vault();
    let owner = session("one");
    let another_owner = session("two");
    let now = Instant::now();
    let handle = vault.register(&owner, image(b"one"), now).unwrap();
    let lease = vault.lease(&owner, &handle, now).unwrap();

    assert_eq!(vault.close_session(&owner), 1);
    assert_eq!(lease.bytes(), b"one");
    assert!(matches!(
        vault.lease(&owner, &handle, now),
        Err(LeaseError::Unavailable)
    ));
    assert_eq!(
        vault.register(&owner, image(b"two"), now),
        Err(RegisterError::SessionClosed)
    );

    let second = vault.register(&another_owner, image(b"two"), now).unwrap();
    vault.evict(&another_owner, &second).unwrap();
    assert!(matches!(
        vault.lease(&another_owner, &second, now),
        Err(LeaseError::Unavailable)
    ));
}

#[test]
fn evicted_leased_bytes_remain_accounted_until_release() {
    let vault = AttachmentVault::new(AttachmentVaultConfig {
        max_image_bytes: 3,
        max_session_images: 2,
        max_session_bytes: 6,
        max_global_images: 1,
        max_global_bytes: 3,
        ttl: Duration::from_secs(1),
    });
    let one = session("one");
    let two = session("two");
    let now = Instant::now();
    let handle = vault.register(&one, image(b"one"), now).unwrap();
    let lease = vault.lease(&one, &handle, now).unwrap();

    vault.evict(&one, &handle).unwrap();
    assert_eq!(
        vault.register(&two, image(b"two"), now),
        Err(RegisterError::GlobalImageLimit { limit: 1 })
    );

    drop(lease);
    assert!(vault.register(&two, image(b"two"), now).is_ok());
}

#[test]
fn fresh_vault_has_no_previous_handle_record() {
    let owner = session("one");
    let now = Instant::now();
    let handle = vault().register(&owner, image(b"one"), now).unwrap();

    assert!(matches!(
        vault().lease(&owner, &handle, now),
        Err(LeaseError::Unknown)
    ));
}

#[test]
fn concurrent_registration_obeys_global_quota() {
    let vault = Arc::new(AttachmentVault::new(AttachmentVaultConfig {
        max_image_bytes: 4,
        max_session_images: 8,
        max_session_bytes: 32,
        max_global_images: 1,
        max_global_bytes: 4,
        ttl: Duration::from_secs(1),
    }));
    let now = Instant::now();
    let joins: Vec<_> = (0..8)
        .map(|number| {
            let vault = Arc::clone(&vault);
            thread::spawn(move || {
                vault.register(&session(&format!("s-{number}")), image(b"one"), now)
            })
        })
        .collect();

    let successes = joins
        .into_iter()
        .map(|join| join.join().unwrap())
        .filter(Result::is_ok)
        .count();
    assert_eq!(successes, 1);
}
