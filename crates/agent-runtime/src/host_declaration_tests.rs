//! 122 的 native 验收：`agent-wasm` 是独立 workspace + wasm32 目标，
//! `cargo test --workspace` 覆盖不到它——「声明 JSON → 工具表」这一步做成纯函数
//! 放在这个 crate，唯一的理由就是**让它能被自动化钉住**（119 §八「native 可测优先」）。
//!
//! 前两条是红线 11 的证据面，剩下的是「拒绝而不是 sanitize」那条纪律的反向锁。

use serde_json::json;

use super::*;

/// 验收第一条：**同一份 JSON 转 1000 次，`specs()` 的序列化逐字节相同**。
///
/// 这不是在测 `serde_json` 是不是确定性的，是在测这条路上没有任何一处偷偷掺进
/// 时钟、随机 id、`HashMap` 迭代序或者插入序（红线 11 的四种典型漏法）。
#[test]
fn the_same_declaration_yields_the_same_bytes_a_thousand_times() {
    let declaration = json!({
        "tools": [
            { "name": "web:crm/lookup", "description": "按客户 ID 查 CRM 档案。",
              "schema": { "type": "object", "properties": { "id": { "type": "string" }, "全名": { "type": "string" } } },
              "reversibility": "pure" },
            { "name": "desk:clipboard/write", "description": "写系统剪贴板。" },
            { "name": "web:host/callback-probe", "description": "验收脚手架。", "reversibility": "pure" }
        ]
    })
    .to_string();

    let first = declared_table_bytes(&declaration).expect("该被接受");
    for round in 1..1000 {
        assert_eq!(
            declared_table_bytes(&declaration).expect("该被接受"),
            first,
            "第 {round} 次转出来的字节跟第一次不一样"
        );
    }
}

/// 验收第二条：**字段顺序被打乱的两份 JSON 转出来相同**。
///
/// 三处顺序全打乱：工具对象里四个字段的书写序、`schema` 对象里键的书写序、
/// `tools` 数组里两条工具的先后。三处都不该漏进 prompt——前两处由
/// `ToolSpec` 的 Rust 字段序与 `serde_json::Map` 的 `BTreeMap` 后端定死，
/// 第三处由 `ToolTable::with_host_tools` 的 `sort_by` 排掉。
#[test]
fn field_and_array_order_in_the_declaration_never_reaches_the_prompt() {
    let one = json!({
        "tools": [
            { "name": "web:a/one", "description": "第一条。",
              "schema": { "type": "object", "properties": { "alpha": { "type": "string" } }, "additionalProperties": false },
              "reversibility": "pure" },
            { "name": "web:b/two", "description": "第二条。", "reversibility": "reversible" }
        ]
    })
    .to_string();
    let shuffled = json!({
        "tools": [
            { "reversibility": "reversible", "description": "第二条。", "name": "web:b/two" },
            { "reversibility": "pure",
              "schema": { "additionalProperties": false, "properties": { "alpha": { "type": "string" } }, "type": "object" },
              "description": "第一条。", "name": "web:a/one" }
        ]
    })
    .to_string();

    assert_ne!(
        one, shuffled,
        "两份输入本身必须是不同的字节，不然这条测试是空的"
    );
    assert_eq!(
        declared_table_bytes(&one).expect("该被接受"),
        declared_table_bytes(&shuffled).expect("该被接受"),
    );
}

/// ⚠️ `description` 进 prompt，**解析层不许做任何规范化**：首尾空白、大小写、
/// 结尾有没有句号、换行——原样进，原样出。这条错了不会报错，只会静默改掉模型
/// 看见的字节。
#[test]
fn the_description_is_carried_over_byte_for_byte() {
    let raw = "  Leading and trailing spaces, no period, MiXeD case\n第二行  ";
    let declaration = json!({ "tools": [ { "name": "web:x/y", "description": raw } ] }).to_string();

    let tools = host_tools_from_declaration(&declaration).expect("该被接受");
    assert_eq!(&*tools[0].0.description, raw);
}

/// `schema` 同理原样收下——键序由 `serde_json` 的 `BTreeMap` 后端归一，内容一个
/// 字节不改写。
#[test]
fn the_schema_is_taken_as_is_and_defaults_to_a_bare_object() {
    let schema = json!({ "type": "object", "properties": { "id": { "type": "string" } } });
    let declaration = json!({ "tools": [ { "name": "web:x/y", "schema": schema } ] }).to_string();
    let tools = host_tools_from_declaration(&declaration).expect("该被接受");
    assert_eq!(*tools[0].0.schema, schema);

    // 缺省值跟 `agent-server` 那侧同一个：一个不吃参数的工具。
    let bare =
        host_tools_from_declaration(&json!({ "tools": [ { "name": "web:x/y" } ] }).to_string())
            .expect("该被接受");
    assert_eq!(*bare[0].0.schema, json!({ "type": "object" }));
}

/// 声明了就用；**没说落保守的 `Irreversible`**（HOST-CAPABILITIES §五：「没说」
/// 不能推定为「安全」）。
#[test]
fn a_declared_reversibility_is_used_and_a_missing_one_falls_conservative() {
    let declaration = json!({
        "tools": [
            { "name": "web:a/pure", "reversibility": "pure" },
            { "name": "web:b/rev", "reversibility": "reversible" },
            { "name": "web:c/irr", "reversibility": "irreversible" },
            { "name": "desk:d/unsaid" }
        ]
    })
    .to_string();

    let levels: Vec<Reversibility> = host_tools_from_declaration(&declaration)
        .expect("该被接受")
        .into_iter()
        .map(|(_, level)| level)
        .collect();
    assert_eq!(
        levels,
        vec![
            Reversibility::Pure,
            Reversibility::Reversible,
            Reversibility::Irreversible,
            Reversibility::Irreversible,
        ],
        "最后那个没声明的必须是保守值"
    );
}

/// 拼法是**小写**。PascalCase（`agent_core::Reversibility` 的 serde 拼法）在宿主面
/// 不认——两种拼法都认会让「协议形状」变成两份。
#[test]
fn reversibility_is_lowercase_on_the_wire() {
    let pascal = json!({ "tools": [ { "name": "web:x/y", "reversibility": "Pure" } ] }).to_string();
    assert_eq!(
        host_tools_from_declaration(&pascal),
        Err(HostDeclarationError::Malformed)
    );
}

/// **反向锁**：服务端前缀、无前缀、空名一律拒——这是 `tools.rs` 那条「从空表起步，
/// 不靠名称黑名单回减」的纪律第一次被外部输入考验。
#[test]
fn server_side_and_prefixless_names_are_rejected() {
    for name in [
        "srv:x/y",
        "mcp:everything/echo",
        "nopfx",
        "",
        "web/x",
        "WEB:x",
        " web:x",
    ] {
        let declaration = json!({ "tools": [ { "name": name } ] }).to_string();
        assert_eq!(
            host_tools_from_declaration(&declaration),
            Err(HostDeclarationError::ToolPrefix {
                name: name.to_string()
            }),
            "{name:?} 该因为前缀被拒"
        );
    }
}

/// 前缀对了，前缀之后照样过白名单：空、空格、冒号、点、非 ASCII、换行、超长。
#[test]
fn the_part_after_the_prefix_is_whitelisted() {
    let too_long = format!("web:{}", "a".repeat(MAX_TOOL_NAME_LEN));
    for name in [
        "web:",
        "desk:",
        "web:a b",
        "web:a:b",
        "web:a.b",
        "web:客户",
        "web:a\nb",
        &too_long,
    ] {
        let declaration = json!({ "tools": [ { "name": name } ] }).to_string();
        assert!(
            matches!(
                host_tools_from_declaration(&declaration),
                Err(HostDeclarationError::ToolNameShape { .. })
            ),
            "{name:?} 该因为字符集/长度被拒"
        );
    }
    // 边界：正好 128 字节合法。
    let exactly_max = format!("web:{}", "a".repeat(MAX_TOOL_NAME_LEN - 4));
    let declaration = json!({ "tools": [ { "name": exactly_max } ] }).to_string();
    assert!(host_tools_from_declaration(&declaration).is_ok());
}

/// 重名整份拒，不做「后来居上」——声明方自己都没想清楚要哪个，替它选一个只会把
/// 问题推到运行时。
#[test]
fn duplicate_names_are_rejected() {
    let declaration =
        json!({ "tools": [ { "name": "web:a/b" }, { "name": "web:a/b" } ] }).to_string();
    assert_eq!(
        host_tools_from_declaration(&declaration),
        Err(HostDeclarationError::DuplicateTool {
            name: "web:a/b".to_string()
        })
    );
}

/// **一条不合法 = 整份拒**，不是「跳过坏的那条、剩下的照用」：声明方拿到的表会跟
/// 它以为自己声明的那份不一样，而那份差异就在 prompt 最前面。
#[test]
fn one_bad_entry_rejects_the_whole_declaration() {
    let declaration = json!({
        "tools": [ { "name": "web:good/one" }, { "name": "srv:bad/two" }, { "name": "web:good/three" } ]
    })
    .to_string();
    assert!(host_tools_from_declaration(&declaration).is_err());
}

/// 空声明合法——不写 `tools` 和写空数组是一回事，下游一路空操作。
#[test]
fn an_empty_declaration_is_an_empty_vec() {
    for text in ["{}", r#"{"tools":[]}"#] {
        assert_eq!(host_tools_from_declaration(text), Ok(Vec::new()));
    }
}

/// 认不得的字段忽略、不报错（宿主比运行时先升级是常态）。**`skills` 与
/// `disable_builtin` 落在这一条里**：本模块只管 `tools`，理由见模块文档。
#[test]
fn unknown_fields_are_ignored() {
    let declaration = json!({
        "tools": [ { "name": "web:x/y", "future_field": 1 } ],
        "skills": [ { "id": "crm-flow" } ],
        "disable_builtin": [ "srv:shell/exec" ]
    })
    .to_string();
    let tools = host_tools_from_declaration(&declaration).expect("该被接受");
    assert_eq!(tools.len(), 1);
}

/// 整段不是 JSON / 形状不对 → 一条错误，不 panic。
///
/// **`[…]` 也在这一条里**：少写一层 `{"tools": …}` 外壳时，serde 默认会把裸数组
/// 当成「按序填结构体字段」而静默解析成空声明——一张空表加零条错误，页面这边一声
/// 不吭。`parse_shape` 专门堵的就是它。
#[test]
fn malformed_json_is_an_error_not_a_panic() {
    for text in [
        "",
        "not json",
        "[]",
        r#"[{"name":"web:x/y"}]"#,
        r#"{"tools":{}}"#,
        r#"{"tools":[3]}"#,
    ] {
        assert_eq!(
            host_tools_from_declaration(text),
            Err(HostDeclarationError::Malformed),
            "{text:?} 该被当成形状错误"
        );
    }
}

/// 错误文案要说得清哪一项，且**不把任意长的输入原样弹回去**。
#[test]
fn the_message_names_the_offending_item_and_stays_bounded() {
    let message = host_tools_from_declaration(
        &json!({ "tools": [ { "name": "srv:crm/lookup" } ] }).to_string(),
    )
    .unwrap_err()
    .to_string();
    assert!(message.contains("srv:crm/lookup"), "{message}");
    assert!(message.contains("web:"), "{message}");

    let huge = format!("srv:{}", "x".repeat(10_000));
    let message =
        host_tools_from_declaration(&json!({ "tools": [ { "name": huge } ] }).to_string())
            .unwrap_err()
            .to_string();
    assert!(
        message.len() < 400,
        "错误文案不该把输入原样弹回去：{} 字节",
        message.len()
    );
    assert!(message.contains('…'), "截断该留个记号：{message}");
}
