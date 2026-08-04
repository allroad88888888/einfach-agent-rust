//! issue 074 验收：一个 server 的 `tools/list` 回包里两项同名，`McpClient::list_tools`
//! 必须在这一跳拦掉——**保留第一条、后来的整条丢弃**，并留下能到达部署方的告警
//! （server id + 重复的工具名 + 丢了几条）。整条链在这之前一处判重都没有：`specs`
//! 是 `Vec`、两条同名 spec 都会进 prompt；可逆性走 `BTreeMap::insert` 是后来居上——
//! 于是模型看第一份说明书、undo 屏障用第二份的可逆性。本文件只测 `list_tools` 这一跳
//! 本身（`loader::connect_stdio` 怎么把告警汇进 `LoadOutcome::warnings` 是
//! `mcp_loader_044.rs` 那层的事，不在这里重复）。
//!
//! 用一段 `sh` 脚本假扮 MCP server（同 `handshake_translate_042.rs` 的手法），零网络
//! 依赖、确定可判定。

use std::time::Duration;

use agent_core::Reversibility;
use agent_mcp::{McpClient, McpError};

fn connect(script: &str) -> Result<McpClient, McpError> {
    McpClient::connect(
        "sh",
        &["-c".to_string(), script.to_string()],
        &[],
        "agent-mcp-074-test",
        "0.0.0",
        Duration::from_secs(5),
    )
}

/// 握手固定回应，`tools_json` 是 `tools/list` result 里 `tools` 数组的内容（不含
/// 外层 `{"tools": [...]}`）。**必须是单行**——`read_line` 按 `\n` 切帧
/// （newline-delimited JSON-RPC），`tools_json` 里混进换行会把一帧切成两半。
fn server_script(tools_json: &str) -> String {
    assert!(!tools_json.contains('\n'), "tools_json 必须是单行，否则破坏 newline-delimited 分帧");
    format!(
        "read l1\n\
         printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{}}}}}}'\n\
         read l2\n\
         read l3\n\
         printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"tools\":[{tools_json}]}}}}'\n"
    )
}

/// 核心验收：同 server 内两项同名，`readOnlyHint` 故意写得不一样（第一条 `true`，
/// 第二条缺失）——不这样写两条一模一样，「保留第一条」这件事就不可判定。
/// `list_tools` 必须只吐一条，且可逆性是**第一条**的那一档（`Pure`），不是后来居上
/// 落到 `Irreversible`。
#[test]
fn duplicate_tool_name_keeps_first_spec_and_first_reversibility() {
    let tools_json = r#"{"name":"echo","description":"first","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},{"name":"echo","description":"second","inputSchema":{"type":"object"}}"#;
    let mut client = connect(&server_script(tools_json)).unwrap();
    let (tools, warnings) = client.list_tools("fakesrv", Duration::from_secs(5)).unwrap();

    assert_eq!(tools.len(), 1, "两条同名，只该留一条");
    assert_eq!(&*tools[0].0.name, "mcp:fakesrv/echo");
    assert_eq!(&*tools[0].0.description, "first", "留下的该是第一条的 spec，不是第二条");
    assert_eq!(tools[0].1, Reversibility::Pure, "可逆性该是第一条的那一档，不是后来居上");

    assert_eq!(warnings.len(), 1, "该有且只有一条告警");
    assert_eq!(warnings[0].server_id, "fakesrv", "告警必须点名 server id");
    assert_eq!(warnings[0].tool_name, "echo", "告警必须点名重复的工具名");
    assert_eq!(warnings[0].dropped, 1, "告警必须点名丢了几条");

    // 报文本身也要含 server id 与重复的工具名——不能只靠结构体字段，Display 出来的
    // 文案(部署方实际看到的那句话)里这两样也不能少。
    let text = warnings[0].to_string();
    assert!(text.contains("fakesrv"), "告警文案缺 server id: {text}");
    assert!(text.contains("echo"), "告警文案缺重复的工具名: {text}");
}

/// 同一个名字出现三次：保留第一条，丢弃的两条都算进同一条告警的 `dropped`。
#[test]
fn same_name_three_times_drops_two_into_one_warning() {
    let tools_json = r#"{"name":"dup","description":"a","inputSchema":{"type":"object"}},{"name":"dup","description":"b","inputSchema":{"type":"object"}},{"name":"dup","description":"c","inputSchema":{"type":"object"}}"#;
    let mut client = connect(&server_script(tools_json)).unwrap();
    let (tools, warnings) = client.list_tools("fakesrv", Duration::from_secs(5)).unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(&*tools[0].0.description, "a", "留下的该是最早那条");
    assert_eq!(warnings.len(), 1, "同一个名字只应产出一条告警，不是每次撞名都开一条");
    assert_eq!(warnings[0].dropped, 2, "丢了两条(第二、第三条)");
}

/// 不误伤：同一批里名字都不同，照常全过、零告警。
#[test]
fn distinct_names_all_kept_with_no_warnings() {
    let tools_json = r#"{"name":"a","inputSchema":{"type":"object"}},{"name":"b","inputSchema":{"type":"object"}},{"name":"c","inputSchema":{"type":"object"}}"#;
    let mut client = connect(&server_script(tools_json)).unwrap();
    let (tools, warnings) = client.list_tools("fakesrv", Duration::from_secs(5)).unwrap();

    let names: Vec<String> = tools.iter().map(|(s, _)| s.name.to_string()).collect();
    assert_eq!(names, vec!["mcp:fakesrv/a", "mcp:fakesrv/b", "mcp:fakesrv/c"]);
    assert!(warnings.is_empty(), "名字都不同，不该有告警");
}

/// 不误伤：空数组照常——零工具、零告警，不 panic、不报错。
#[test]
fn empty_tools_list_is_fine() {
    let mut client = connect(&server_script("")).unwrap();
    let (tools, warnings) = client.list_tools("fakesrv", Duration::from_secs(5)).unwrap();
    assert!(tools.is_empty());
    assert!(warnings.is_empty());
}
