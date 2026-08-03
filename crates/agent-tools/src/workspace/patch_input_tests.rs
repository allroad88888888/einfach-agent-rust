use super::{ContentGuard, PatchOperation, parse};
use serde_json::json;

#[test]
fn parses_closed_operations_and_defaults_replace_count() {
    let input = parse(&json!({
        "operations": [
            { "type": "add_file", "path": "new.txt", "content": "new" },
            { "type": "delete_file", "path": "old.txt", "oldContent": "old" },
            {
                "type": "replace",
                "path": "note.txt",
                "oldText": "before",
                "newText": "after"
            },
            {
                "type": "overwrite_file",
                "path": "hash.txt",
                "content": "replacement",
                "expectedContentHash":
                    "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            }
        ],
        "dryRun": true
    }))
    .unwrap();

    assert!(input.dry_run);
    assert!(matches!(
        input.operations[0],
        PatchOperation::AddFile { .. }
    ));
    assert!(matches!(
        input.operations[1],
        PatchOperation::DeleteFile {
            guard: ContentGuard::Exact(_),
            ..
        }
    ));
    assert!(matches!(
        input.operations[2],
        PatchOperation::Replace {
            expected_replacements: 1,
            ..
        }
    ));
    assert!(matches!(
        input.operations[3],
        PatchOperation::OverwriteFile {
            guard: ContentGuard::Sha256(_),
            ..
        }
    ));
}

#[test]
fn rejects_ambiguous_or_unprotected_mutations() {
    let no_guard = parse(&json!({
        "operations": [{ "type": "delete_file", "path": "old.txt" }]
    }))
    .unwrap_err();
    assert_eq!(&*no_guard.code, "bad_input");

    let duplicate_path = parse(&json!({
        "operations": [
            { "type": "add_file", "path": "same.txt", "content": "x" },
            { "type": "replace", "path": "same.txt", "oldText": "x", "newText": "y" }
        ]
    }))
    .unwrap_err();
    assert_eq!(&*duplicate_path.code, "bad_input");

    let zero_replacements = parse(&json!({
        "operations": [{
            "type": "replace",
            "path": "note.txt",
            "oldText": "x",
            "newText": "y",
            "expectedReplacements": 0
        }]
    }))
    .unwrap_err();
    assert_eq!(&*zero_replacements.code, "bad_input");
}

#[test]
fn rejects_unknown_fields_and_bad_hashes() {
    let unknown = parse(&json!({
        "operations": [{
            "type": "add_file",
            "path": "new.txt",
            "content": "x",
            "executable": true
        }]
    }))
    .unwrap_err();
    assert_eq!(&*unknown.code, "bad_input");

    let bad_hash = parse(&json!({
        "operations": [{
            "type": "overwrite_file",
            "path": "note.txt",
            "content": "new",
            "expectedContentHash": "sha256:UPPERCASE"
        }]
    }))
    .unwrap_err();
    assert_eq!(&*bad_hash.code, "bad_input");
}
