//! 两个并发调用者写同一文件时的公开接口验收。

mod support;

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};
use support::TestRoot;

fn inspect_revision(executor: &ToolExecutor) -> String {
    let output = executor
        .execute("srv:fs/inspect", &json!({ "path": "note.txt" }))
        .unwrap();
    serde_json::from_str::<Value>(&output).unwrap()["revision"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn simultaneous_writes_have_one_winner_and_one_revision_conflict() {
    let root = TestRoot::new("workspace-tools-concurrent-write");
    root.write("note.txt", "before");
    let executor = Arc::new(ToolExecutor::new(root.path()).unwrap());
    let revision = inspect_revision(&executor);
    let start = Arc::new(Barrier::new(3));

    let mut workers = Vec::new();
    for content in ["from-a", "from-b"] {
        let executor = Arc::clone(&executor);
        let revision = revision.clone();
        let start = Arc::clone(&start);
        workers.push(std::thread::spawn(move || {
            start.wait();
            executor.execute(
                "srv:fs/write_text",
                &json!({
                    "path": "note.txt",
                    "content": content,
                    "expected_revision": revision,
                }),
            )
        }));
    }

    start.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    assert_eq!(
        &*results.into_iter().find_map(Result::err).unwrap().code,
        "conflict"
    );

    let contents = std::fs::read_to_string(root.path().join("note.txt")).unwrap();
    assert!(matches!(contents.as_str(), "from-a" | "from-b"));
}
