//! 一件事：**这次调用的可逆性声明，是一个本仓兑现不了的承诺吗**
//! （决策 199 §七「承诺挡，事实不挡」，落地 202）。
//!
//! # 为什么判据是「事实还是承诺」，不是「能不能交出函数」
//!
//! 199 之前，`/undo` 挡不挡看的是一个枚举标签：宿主说 `pure` 就不挡，说
//! `reversible` 也不挡。199 把行为依据换成了**工具执行完交回来的还原函数**——
//! 撤销外部世界要跑的是函数，标签吹得再好也变不出一个能调用的东西。
//!
//! 于是有两类工具天生尴尬：宿主 `web:`/`desk:` 的执行体在浏览器/桌面进程里，
//! MCP 的执行体在 server 里，**它们结构上交不回一个我们能调用的函数**。
//! 199 §七 初稿据此写成「一律挡」——**那是错的**，而且错法跟 §一 初稿写成
//! `Option<UndoFn>` 一模一样：**把「没碰外部世界」和「碰了但撤不回」压成了同一格**。
//!
//! 「交不出函数」这个理由对 `Undo(f)` 成立，**对 `Nothing` 不成立——因为 `Nothing`
//! 本来就不需要函数**。所以真正的判据是**声明的到底是一个事实还是一个承诺**：
//!
//! | 声明 | 落成 | 为什么 |
//! |---|---|---|
//! | `Pure`（宿主 `pure` / MCP `readOnlyHint: true`） | `StateOnly`，**不挡** | 声明的是「没碰外部世界」这个**事实断言**，不需要任何函数来兑现 |
//! | `Reversible`（宿主 `reversible`） | `Blocked`，**挡** | 声明的是「有补偿动作」这个**承诺**，而承诺要兑现就得交出那个结构上交不出来的函数 |
//! | `Irreversible`（含未声明） | `Blocked`，**挡** | 本来就挡，不归本模块管（`dispatch` 分流前那个公共块已经标了） |
//!
//! **事实可以采信，承诺不能空转。** 采信一个我们无法验证的事实断言，跟采信扩展包
//! 作者返回 `Aftermath::Nothing` 是同一个信任级别（`docs/EXTENSIONS.md` §可逆性、
//! `docs/HOST-CAPABILITIES.md` §五 两条既有判据都指向采信）。而采信一个**结构上不
//! 可能被兑现**的承诺，等于把「有人会补偿」当成「已经补偿了」——那正是 199 现状清账
//! 里 CRM 草稿留在原地、`/undo` 却一声不吭的那个场景。
//!
//! **两条推论**（199 §七 要求写下来的）：
//!
//! 1. `ask_user_question` 这类工具**不受影响**：`Web` + `Pure`，字面上不可能有副作用，
//!    挡住它保护不了任何东西，只是让「模型问了一句话」那一轮撤不掉。
//! 2. **决策 22 不被反转**：MCP `readOnlyHint: true → Pure → 不挡` 是 M6 已发布的行为，
//!    原样保留。
//!
//! # 为什么这个判据要单独成一个函数
//!
//! 它有**两个消费者，而且必须给同一个答案**：
//!
//! | 消费者 | 用它干什么 |
//! |---|---|
//! | [`crate::dispatch`] 的第四路 / 第五路 | 行为：派发那一刻 `mark_no_undo`，让结果那条 entry 落 `Blocked` |
//! | CLI（`agent_cli::print`）与 Web（`packages/web/src/render/tool.ts`）的工具卡片 | 诚实：**只有这一格**要在 `reversibility` 后面补一句「本仓不代为补偿」 |
//!
//! 两处各写一遍就是第二份真相：行为改了而文案没跟上，用户看到的仍然是一个孤零零的
//! `Reversible`——**那正是 199 现状清账里骗人的那个字**。反过来也一样要命：给 `Pure`
//! 也挂上那句免责声明，等于说一件它从没承诺过的事没被兑现。

use agent_core::{Reversibility, ToolCallRequest};

/// 这次调用声明的可逆性，是一个**本仓结构上兑现不了的承诺**吗。
///
/// `true` ⇒ 这条 entry 该落 [`Undoability::Blocked`](agent_core::Undoability::Blocked)，
/// 且显示时要说明本仓不代为补偿。判据与三格取舍见模块文档。
pub fn is_unkeepable_promise(request: &ToolCallRequest) -> bool {
    // 写成穷举 `match` 而不是 `matches!(…, Reversible)`：`Reversibility` 哪天加第四
    // 档，编译器会在这里逼一个决定，而不是让新档静默落进「不挡」——跟 `dispatch`
    // 的 `match effect` 不加 `_` 是同一条规矩。
    match request.reversibility {
        // 事实断言：采信。它没承诺任何补偿动作，所以「交不出函数」挡不住它。
        Reversibility::Pure => false,
        // 承诺：只有在执行体够不着的时候才是空的。同一个 `reversible` 声明落在
        // `srv:`/`ext:` 上不归这里管——那些工具**能**交回还原函数（201 的
        // `Aftermath::Undo`），交没交由执行体自己说了算。
        Reversibility::Reversible => executes_out_of_process(request),
        // 已经是「撤不回去」的事实，`dispatch` 分流前那个公共块早就标了。
        // 这里返回 `false` 不是「不挡」，是「不归本模块管」——显示层也据此不给它
        // 挂「不代为补偿」的后缀：它从没承诺过补偿。
        Reversibility::Irreversible => false,
    }
}

/// 执行体在**别的进程**里吗——承诺兑现不了的结构性原因。
fn executes_out_of_process(request: &ToolCallRequest) -> bool {
    // `location` 已经把 `web:`/`desk:` 与标准集那几个裸名（`ask_user_question` 等，
    // `tool_table_names::location_of`）一起圈进来了，所以这里不重复判前缀。
    //
    // MCP 另判一次前缀而不是靠 `location`：`mcp:` 的 `location` 是
    // `Location::Server`（子进程往返在宿主本地跑完，不需要远端回传，043 的裁决），
    // 可它的**执行体**在 server 那边——「在不在本进程」和「谁执行的」在这一路上
    // 分叉了，所以要两个条件。
    //
    // MCP 这一半今天**射不中任何东西**：`agent_mcp::translate` 只产出 `Pure` 和
    // `Irreversible`，从不产出 `Reversible`。留着不是占位——`ToolTable::with_mcp`
    // 收的是任意 `(ToolSpec, Reversibility)`，而 `docs/MCP.md` §翻译规则 明写着
    // 「除非本地配置显式标注」这条口子。哪天真有人从本地配置标出一个 `reversible`
    // 的 MCP 工具，它必须跟宿主那一格一样挡，而不是靠「碰巧没人这么填」保平安。
    request.location.is_remote() || request.tool.starts_with("mcp:")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::Location;

    use super::*;

    fn request(tool: &str, location: Location, reversibility: Reversibility) -> ToolCallRequest {
        ToolCallRequest {
            tool: Arc::from(tool),
            input: Arc::new(serde_json::json!({})),
            location,
            reversibility,
        }
    }

    /// 承诺那一格：执行体在别的进程里 + 声明 `reversible` ⇒ 挡。
    /// **这一格正是 199 存在的理由**——声明了补偿动作，却没有任何人会执行它。
    #[test]
    fn a_reversible_declaration_from_another_process_is_an_unkeepable_promise() {
        for (tool, location) in [
            ("web:crm/draft", Location::Web),
            ("desk:clipboard/write", Location::Desktop),
            // MCP 的 `location` 是 `Server`，判据落在前缀那一半。
            ("mcp:everything/writeFile", Location::Server),
        ] {
            assert!(
                is_unkeepable_promise(&request(tool, location, Reversibility::Reversible)),
                "{tool} 声明了补偿动作，而它结构上交不出那个函数"
            );
        }
    }

    /// 事实那一格：同样够不着的执行体，声明 `pure` ⇒ **不挡**。
    ///
    /// 这条是 199 §七 修正的钉子。初稿的「一律挡」会把这一格也挡上——而挡住一个
    /// 「没碰外部世界」的声明保护不了任何东西，只是让那一轮撤不掉。`ask_user_question`
    /// 是最直接的证据，所以它在这里逐字出现。
    #[test]
    fn a_pure_declaration_is_a_fact_we_take_at_face_value() {
        for (tool, location) in [
            ("web:demo/page-title", Location::Web),
            ("ask_user_question", Location::Web),
            ("desk:screen/size", Location::Desktop),
            // 决策 22 不被反转：`readOnlyHint: true` 落 `Pure`，M6 起就不挡。
            ("mcp:everything/echo", Location::Server),
        ] {
            assert!(
                !is_unkeepable_promise(&request(tool, location, Reversibility::Pure)),
                "{tool} 声明的是「没碰外部世界」这个事实，不需要函数来兑现"
            );
        }
    }

    /// `Irreversible` 不归本模块管：它早就被 `dispatch` 分流前那个公共块标了，
    /// 而且它从没承诺过补偿——显示层据此**不**给它挂「不代为补偿」的后缀。
    #[test]
    fn an_irreversible_declaration_is_not_this_modules_business() {
        assert!(!is_unkeepable_promise(&request(
            "web:crm/send-invoice",
            Location::Web,
            Reversibility::Irreversible
        )));
    }

    /// 本进程内执行的工具**一格都不归这里管**，连 `reversible` 也不：它们**能**
    /// 交回还原函数（201 的 `Aftermath::Undo`），交没交由执行体自己说了算。
    /// 判成 `true` 会把 `srv:agent/spawn` 变成屏障——`tool_table_names.rs` 早就
    /// 论证过那样会让「拆了任务的那一轮」一律撤不掉。
    #[test]
    fn in_process_tools_are_never_an_unkeepable_promise() {
        for tool in ["srv:agent/spawn", "srv:fs/read", "ext:stats/report"] {
            for reversibility in [
                Reversibility::Pure,
                Reversibility::Reversible,
                Reversibility::Irreversible,
            ] {
                assert!(
                    !is_unkeepable_promise(&request(tool, Location::Server, reversibility)),
                    "{tool}（{reversibility:?}）在本进程里跑，还原归执行体自己交代"
                );
            }
        }
    }
}
