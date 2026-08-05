//! `srv:workspace/find_test_lint_commands` 的发现边界：只读 manifest，返回 argv 候选。

mod support;

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use support::TestRoot;

fn discover(root: &TestRoot) -> Value {
    let output = ToolExecutor::new(root.path())
        .unwrap()
        .execute("srv:workspace/find_test_lint_commands", &json!({}))
        .unwrap();
    serde_json::from_str(&output).unwrap()
}

fn has_command(result: &Value, kind: &str, argv: &[&str], cwd: &str, origin: &str) -> bool {
    result["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| {
            command["kind"] == kind
                && command["argv"] == json!(argv)
                && command["cwd"] == cwd
                && command["origin"] == origin
        })
}

#[test]
fn discovers_common_manifests_as_safe_argv_candidates_without_running_them() {
    let root = TestRoot::new("command-discovery-common");
    root.write(
        "Cargo.toml",
        "[package]\nname = 'demo'\nversion = '0.1.0'\n",
    );
    root.write(
        "web/package.json",
        r#"{"packageManager":"pnpm@9.0.0","scripts":{"test":"vitest run && rm -rf /","lint":"eslint .","test:unit":"vitest run unit","build":"vite build"}}"#,
    );
    root.write(
        "python/pyproject.toml",
        "[project]\nname = 'demo'\n[tool.pytest.ini_options]\n[tool.ruff]\n",
    );
    root.write("service/go.mod", "module example.test/service\ngo 1.22\n");
    root.write(
        "node_modules/ignored/package.json",
        r#"{"scripts":{"test":"bad"}}"#,
    );

    let result = discover(&root);
    assert_eq!(result["truncated"], json!(false));
    assert_eq!(
        result["manifests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|manifest| manifest["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "Cargo.toml",
            "python/pyproject.toml",
            "service/go.mod",
            "web/package.json"
        ]
    );
    assert!(has_command(
        &result,
        "test",
        &["cargo", "test"],
        ".",
        "inferred"
    ));
    assert!(has_command(
        &result,
        "lint",
        &["go", "vet", "./..."],
        "service",
        "inferred"
    ));
    assert!(has_command(
        &result,
        "test",
        &["python", "-m", "pytest"],
        "python",
        "inferred"
    ));
    assert!(has_command(
        &result,
        "lint",
        &["python", "-m", "ruff", "check", "."],
        "python",
        "inferred"
    ));
    assert!(has_command(
        &result,
        "test",
        &["pnpm", "run", "test"],
        "web",
        "declared"
    ));
    assert!(has_command(
        &result,
        "lint",
        &["pnpm", "run", "lint"],
        "web",
        "declared"
    ));
    assert!(!serde_json::to_string(&result).unwrap().contains("rm -rf"));
    assert!(
        result["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|command| {
                command["argv"].as_array().unwrap().iter().all(|argument| {
                    !argument
                        .as_str()
                        .unwrap()
                        .contains(['&', '|', ';', '<', '>', '`', '$'])
                })
            })
    );
}

#[test]
fn ignores_symlinked_manifests_and_rejects_any_input_parameter() {
    let root = TestRoot::new("command-discovery-boundary");
    root.write("Cargo.toml", "[package]\nname = 'demo'\n");
    let executor = ToolExecutor::new(root.path()).unwrap();
    let error = executor
        .execute(
            "srv:workspace/find_test_lint_commands",
            &json!({"path":"../"}),
        )
        .unwrap_err();
    assert_eq!(&*error.code, "bad_input");

    #[cfg(unix)]
    {
        let outside = root.path().parent().unwrap().join(format!(
            "outside-{}",
            root.path().file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(&outside, r#"{"scripts":{"test":"unsafe"}}"#).unwrap();
        std::os::unix::fs::symlink(&outside, root.path().join("package.json")).unwrap();
        let result: Value = serde_json::from_str(
            &executor
                .execute("srv:workspace/find_test_lint_commands", &json!({}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["manifests"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_file(outside);
    }
}

#[test]
fn manifest_cap_is_deterministic_and_keeps_shallow_paths_first() {
    let root = TestRoot::new("command-discovery-cap");
    root.write("package.json", r#"{"scripts":{"test":"node --test"}}"#);
    for index in 0..20 {
        root.write(
            &format!("apps/app-{index:02}/package.json"),
            r#"{"scripts":{"test":"node --test"}}"#,
        );
    }
    let first = serde_json::to_string(&discover(&root)).unwrap();
    let second = serde_json::to_string(&discover(&root)).unwrap();
    assert_eq!(first, second);

    let result: Value = serde_json::from_str(&first).unwrap();
    let manifests = result["manifests"].as_array().unwrap();
    assert_eq!(manifests.len(), 16);
    assert_eq!(manifests[0]["path"], json!("package.json"));
    assert_eq!(result["truncated"], json!(true));
    assert!(
        result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("cap"))
    );
}
