//! 会话 id 的白名单：`[A-Za-z0-9_-]`、非空、≤128，**拒绝而不 sanitize**。
//!
//! 114 §范围第 4 条把这条钉死了：会话 id 由宿主给，仍走 055 的白名单。规则的
//! 权威实现在 `agent-server`（`http/routes/sessions.rs::is_valid_client_session_id`），
//! 但那个 crate 是 native-only 的 HTTP 服务，进不了浏览器构建——所以这里是同一
//! 条规则的第二处落点，**逐字对齐**（改一处必须改另一处）。
//!
//! # 为什么是「拒绝」而不是「清洗成合法的」
//!
//! 清洗是静默的多对一映射：`a/b` 和 `a_b` 会落进同一个会话。宿主想按 URL 分
//! 会话是完全合理的需求，但**归一化和取摘要是宿主的事**（114 原文），URL 本身
//! 不是合法 id。在这里替它清洗，等于替它做了一个它看不见、也无法验证的决定。
//!
//! # 这条规则在浏览器里额外扛的一件事
//!
//! 会话 id 会被拼进 IndexedDB 的库名（`crate::db`）。白名单同时保证了那次拼接
//! 不需要任何转义。

/// 见模块文档。
const MAX_LEN: usize = 128;

/// 合法就原样还回来，不合法给一句能照着改的错——**不回显整个 id**（它可能很长、
/// 也可能是宿主拿别的东西拼的），只说规则。
pub(crate) fn validate(id: &str) -> Result<&str, String> {
    if id.is_empty() {
        return Err("会话 id 不能为空".to_string());
    }
    if id.len() > MAX_LEN {
        return Err(format!(
            "会话 id 最长 {MAX_LEN} 字节，这个有 {} 字节",
            id.len()
        ));
    }
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(
            "会话 id 只允许 [A-Za-z0-9_-]。要按 URL 分会话的话，归一化和取摘要是宿主的事——URL 本身不是合法 id。"
                .to_string(),
        );
    }
    Ok(id)
}
