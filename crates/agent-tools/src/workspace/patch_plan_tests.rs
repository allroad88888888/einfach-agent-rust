use super::{ObservedFile, build};
use crate::workspace::journal_record::OriginalContents;
use crate::workspace::patch_input::parse;
use crate::workspace::revision::Revision;
use crate::workspace::target_path::parse_workspace_target;
use serde_json::json;
use std::path::Path;

fn observed(path: &str, contents: Option<&str>) -> ObservedFile {
    let original = match contents {
        Some(contents) => OriginalContents::Present(contents.as_bytes().to_vec()),
        None => OriginalContents::Absent,
    };
    let revision = match contents {
        Some(contents) => Revision::for_contents(contents.as_bytes()),
        None => Revision::absent(),
    };
    ObservedFile {
        target: parse_workspace_target(Path::new("."), path).unwrap(),
        original,
        revision,
    }
}

#[test]
fn plans_replace_and_hash_guarded_overwrite() {
    let old_hash = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let input = parse(&json!({
        "operations": [
            { "type": "replace", "path": "note.txt", "oldText": "a", "newText": "b" },
            {
                "type": "overwrite_file", "path": "hash.txt", "content": "next",
                "expectedContentHash": old_hash
            }
        ]
    }))
    .unwrap();
    let staged = build(
        &input.operations,
        vec![
            observed("note.txt", Some("a a")),
            observed("hash.txt", Some("abc")),
        ],
    )
    .unwrap_err();
    assert_eq!(&*staged.code, "conflict");

    let input = parse(&json!({
        "operations": [{
            "type": "replace", "path": "note.txt", "oldText": "a", "newText": "b",
            "expectedReplacements": 2
        }]
    }))
    .unwrap();
    let staged = build(&input.operations, vec![observed("note.txt", Some("a a"))]).unwrap();
    assert_eq!(staged[0].replacement.as_deref(), Some(&b"b b"[..]));
}

#[test]
fn refuses_add_over_existing_file_and_invalid_result_size() {
    let add = parse(&json!({
        "operations": [{ "type": "add_file", "path": "note.txt", "content": "new" }]
    }))
    .unwrap();
    assert_eq!(
        &*build(&add.operations, vec![observed("note.txt", Some("old"))])
            .unwrap_err()
            .code,
        "already_exists"
    );

    let oversized = "x".repeat(crate::workspace::text_file::MAX_TEXT_FILE_BYTES);
    let replace = parse(&json!({
        "operations": [{
            "type": "replace", "path": "note.txt", "oldText": "a", "newText": oversized,
            "expectedReplacements": 2
        }]
    }))
    .unwrap();
    assert_eq!(
        &*build(&replace.operations, vec![observed("note.txt", Some("aa"))])
            .unwrap_err()
            .code,
        "file_too_large"
    );
}
