//! 工具表逐字节稳定（issue 013 验收 4，红线 11）：`builtin_specs()` 顺序固定
//! 为 `[srv:fs/read, srv:fs/list]`，同一份表两次 `serde_json::to_vec` 字节
//! 完全相等。

use agent_tools::builtin_specs;

#[test]
fn builtin_specs_order_is_fixed() {
    let specs = builtin_specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_ref()).collect();
    assert_eq!(names, vec!["srv:fs/read", "srv:fs/list"]);
}

#[test]
fn builtin_specs_serialize_byte_identical_across_calls() {
    let bytes_a = serde_json::to_vec(&builtin_specs()).unwrap();
    let bytes_b = serde_json::to_vec(&builtin_specs()).unwrap();
    assert_eq!(
        bytes_a, bytes_b,
        "红线 11：同一份工具表两次序列化字节必须完全相同，否则前缀缓存每轮全价"
    );
}

#[test]
fn builtin_specs_serialize_byte_identical_across_several_calls() {
    // 多测几轮，降低偶发的内部分配顺序巧合掩盖问题的概率。
    let baseline = serde_json::to_vec(&builtin_specs()).unwrap();
    for _ in 0..5 {
        let bytes = serde_json::to_vec(&builtin_specs()).unwrap();
        assert_eq!(bytes, baseline);
    }
}
