//! 工具的静态描述（`ToolSpec`）与一次具体调用请求（`ToolCallRequest`）。
//!
//! `Location` 与 `Reversibility` 是两个正交维度（决策 7，docs/ROADMAP.md）：
//! 执行在哪、可不可逆是两件独立的事——桌面工具可以不可逆（写剪贴板），服务端
//! 工具可以是纯的（读文件）。不要把它们合并成一个「工具分类」枚举。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// 工具的执行位置。router 靠它决定往哪发：本地直接执行、经 SSE 反向调用 Web
/// 客户端、还是调桌面侧。
///
/// 032：经 `ToolCallRequest.location` 可达，`ts` feature 门后面导出 TS。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Location {
    Server,
    Web,
    Desktop,
}

impl Location {
    /// `Server` 在本进程内执行；`Web` / `Desktop` 要经一次网络往返（服务端推
    /// 事件、等客户端把结果回传）——对 loop 而言这是「立即拿到结果」还是
    /// 「要挂起等」的分野，见 docs/TOOLS.md。
    pub fn is_remote(self) -> bool {
        matches!(self, Location::Web | Location::Desktop)
    }

    /// 工具全名的前缀（如 `"srv:fs/read"`），见 docs/TOOLS.md 里
    /// `ToolDescriptor.name` 的约定。三个前缀在日志和 CLI 输出里要保持稳定，
    /// 所以焊在这一处而不是各处现拼字符串。
    pub fn prefix(self) -> &'static str {
        match self {
            Location::Server => "srv",
            Location::Web => "web",
            Location::Desktop => "desk",
        }
    }
}

/// 一次工具调用的可逆性等级。决定 undo 能不能越过它、崩溃恢复时能不能重发。
/// 判据与「拿不准怎么办」的完整讨论见 docs/TOOLS.md §「reversibility 等级怎么定」。
///
/// **拿不准就填 `Irreversible`。** 判错成 `Pure` 的代价是重复发邮件、重复扣款；
/// 判错成 `Irreversible` 的代价只是多问用户一次「要不要继续」。两个错误的代价
/// 不对称，所以默认值必须落在保守的那一边——这也是 `undo_blocked` 事件存在的
/// 原因：宁可多问，不可错放。
/// 032：经 `ToolCallRequest.reversibility` 可达，`ts` feature 门后面导出 TS。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Reversibility {
    /// 重复执行任意次，外部世界不变（读文件、查询、搜索）。
    Pure,
    /// 有明确且可靠的补偿动作（创建资源，补偿是删除；写入有版本的记录）。
    Reversible,
    /// 其余全部（发邮件、支付、删数据、跑 shell）。
    Irreversible,
}

impl Reversibility {
    /// 仅 `Pure` 可以在崩溃恢复时安全重放——重放等价于「再读一次」，不会有
    /// 副作用累积。
    pub fn is_replayable(self) -> bool {
        matches!(self, Reversibility::Pure)
    }

    /// 仅 `Irreversible` 会挡住 undo：往回走撞上它就停下，推 `undo_blocked`
    /// 事件让用户确认「继续（副作用不回滚）」还是取消（docs/TOOLS.md）。
    /// `Reversible` 有补偿动作，不挡。
    pub fn blocks_undo(self) -> bool {
        matches!(self, Reversibility::Irreversible)
    }
}

/// 一个工具的静态描述——喂给 provider 的工具表里的一项。
///
/// **红线 11**：这个类型一旦装进 `Vec<ToolSpec>`，会被逐字节序列化进 system
/// prompt 最前面，前缀缓存靠逐字节相等判定命中。`schema` 是
/// `Arc<serde_json::Value>`，它的 `Map` 后端默认是 `BTreeMap`（key 按字典序
/// 排）——这是本仓依赖的行为，不是碰巧如此，所以 workspace 顶层的
/// `serde_json` 依赖**显式不开** `preserve_order` 特性（开了 `Map` 就换成
/// `IndexMap`，顺序跟着插入顺序走，前缀会漂，参见根 `Cargo.toml` 注释）。
/// 改这个类型、或改它引用的 `serde_json` 特性开关之前，先看
/// docs/INVARIANTS.md 红线 11。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: Arc<str>,
    pub description: Arc<str>,
    pub schema: Arc<serde_json::Value>,
}

/// 模型发起的一次工具调用请求，携带 router 和 undo 逻辑各自要读的字段。
/// `input` 是未解析的 JSON——参数含义由工具自己的 schema 定义，core 不关心。
///
/// 032：`SessionEvent::ToolExecuting.request` 的类型，`ts` feature 门后面导出
/// TS。`input: Arc<serde_json::Value>` 要 ts-rs 的 `serde-json-impl` feature
/// 才有 TS 形状——落成它自带的影子类型 `JsonValue`（递归的
/// `string | number | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null`），
/// 单独导出到 `generated/serde_json/JsonValue.ts`，不是本仓自定义的类型。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ToolCallRequest {
    pub tool: Arc<str>,
    pub input: Arc<serde_json::Value>,
    pub location: Location,
    pub reversibility: Reversibility,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tool_spec_roundtrip() {
        let spec = ToolSpec {
            name: Arc::from("srv:fs/read"),
            description: Arc::from("read a file"),
            schema: Arc::new(
                json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            ),
        };
        let s = serde_json::to_string(&spec).unwrap();
        assert_eq!(serde_json::from_str::<ToolSpec>(&s).unwrap(), spec);
    }

    #[test]
    fn tool_call_request_roundtrip() {
        let req = ToolCallRequest {
            tool: Arc::from("srv:fs/read"),
            input: Arc::new(json!({"path": "/tmp/a"})),
            location: Location::Server,
            reversibility: Reversibility::Pure,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<ToolCallRequest>(&s).unwrap(), req);
    }

    /// 红线 11 的最小实检：`Vec<ToolSpec>` 序列化两次逐字节相同，即使构造
    /// schema 时故意用两种不同的插入顺序——证明 `serde_json::Value` 的 `Map`
    /// 是 `BTreeMap`（按 key 排序），不是随插入顺序走的 `HashMap`/`IndexMap`。
    #[test]
    fn tool_spec_vec_serializes_byte_identical_regardless_of_insertion_order() {
        // 顺序 A：先 path 后 recursive。
        let mut map_a = serde_json::Map::new();
        map_a.insert("path".to_string(), json!({"type": "string"}));
        map_a.insert("recursive".to_string(), json!({"type": "boolean"}));
        let schema_a = serde_json::Value::Object(map_a);

        // 顺序 B：先 recursive 后 path —— 插入顺序与 A 相反，key 集合相同。
        let mut map_b = serde_json::Map::new();
        map_b.insert("recursive".to_string(), json!({"type": "boolean"}));
        map_b.insert("path".to_string(), json!({"type": "string"}));
        let schema_b = serde_json::Value::Object(map_b);

        let specs_a = vec![
            ToolSpec {
                name: Arc::from("srv:fs/read"),
                description: Arc::from("read a file"),
                schema: Arc::new(schema_a),
            },
            ToolSpec {
                name: Arc::from("srv:fs/write"),
                description: Arc::from("write a file"),
                schema: Arc::new(json!({"type": "object"})),
            },
        ];
        let specs_b = vec![
            ToolSpec {
                name: Arc::from("srv:fs/read"),
                description: Arc::from("read a file"),
                schema: Arc::new(schema_b),
            },
            ToolSpec {
                name: Arc::from("srv:fs/write"),
                description: Arc::from("write a file"),
                schema: Arc::new(json!({"type": "object"})),
            },
        ];

        let bytes_a = serde_json::to_vec(&specs_a).unwrap();
        let bytes_b = serde_json::to_vec(&specs_b).unwrap();
        assert_eq!(
            bytes_a, bytes_b,
            "ToolSpec 的 schema 序列化必须与插入顺序无关（红线 11）"
        );

        // 同一份 specs 序列化两次也必须逐字节相同。
        let bytes_a_again = serde_json::to_vec(&specs_a).unwrap();
        assert_eq!(bytes_a, bytes_a_again);
    }

    /// `Location::is_remote` / `prefix` 的穷举断言。
    #[test]
    fn location_methods_exhaustive() {
        assert!(!Location::Server.is_remote());
        assert!(Location::Web.is_remote());
        assert!(Location::Desktop.is_remote());

        assert_eq!(Location::Server.prefix(), "srv");
        assert_eq!(Location::Web.prefix(), "web");
        assert_eq!(Location::Desktop.prefix(), "desk");
    }

    /// `Reversibility::is_replayable` / `blocks_undo` 的穷举断言。
    #[test]
    fn reversibility_predicates_exhaustive() {
        assert!(Reversibility::Pure.is_replayable());
        assert!(!Reversibility::Reversible.is_replayable());
        assert!(!Reversibility::Irreversible.is_replayable());

        assert!(!Reversibility::Pure.blocks_undo());
        assert!(!Reversibility::Reversible.blocks_undo());
        assert!(Reversibility::Irreversible.blocks_undo());
    }
}
