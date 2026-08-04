use super::WorkspaceTransactionCoordinator;
use crate::workspace::patch_input::parse;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn one_mebib_apply_patch_keeps_a_bounded_result() {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "agent-tools-apply-patch-perf-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let contents = "a".repeat(crate::workspace::text_file::MAX_TEXT_FILE_BYTES);
    std::fs::write(root.join("large.txt"), &contents).unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(&root).unwrap();
    let input = parse(&json!({
        "operations": [{
            "type": "overwrite_file", "path": "large.txt", "content": contents,
            "expectedContentHash":
                "sha256:9bc1b2a288b26af7257a36277ae3816a7d4f16e89c1e7e77d0a5c48bad62b360"
        }]
    }))
    .unwrap();

    let result = coordinator.apply_patch(&input).unwrap();
    assert!(result.change_id.is_none());
    assert_eq!(result.changed_files.len(), 0);
    let _ = std::fs::remove_dir_all(root);
}
