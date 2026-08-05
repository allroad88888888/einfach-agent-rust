//! 命令发现声明必须保持模型可用的闭合空输入 schema。

use agent_tools::find_test_lint_commands_spec;

#[test]
fn command_discovery_schema_is_closed_empty_and_stable() {
    let first = find_test_lint_commands_spec();
    let second = find_test_lint_commands_spec();
    assert_eq!(&*first.name, "find_test_lint_commands");
    assert_eq!(first.schema["type"], "object");
    assert_eq!(first.schema["properties"].as_object().unwrap().len(), 0);
    assert_eq!(first.schema["additionalProperties"], false);
    assert!(first.description.contains("不会执行"));
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap(),
        "工具声明必须逐字节稳定"
    );
}
