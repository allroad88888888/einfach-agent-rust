//! 工具名在 wire 上的转义与还原。
//!
//! 我们的工具全名带命名空间分隔符（`srv:fs/read`，见 docs/TOOLS.md），而
//! OpenAI 系 `function.name` 的惯例字符集是 `[a-zA-Z0-9_-]`。
//! **`probes/results/` 里没有任何冒号/斜杠能过的证据**——三家实测只用过
//! `get_weather` / `get_time` / `lookup_record` / `summarize_region` 这类朴素名，
//! 字符集本身没被探过。两边的代价不对称：名字被拒是 400、整轮请求废掉，
//! 转义的代价只是名字长几个字符。所以按最保守的字符集转义。
//!
//! **转义规则分两档，先试可读的那档：**
//!
//! 1. 可读档：只把 `[a-zA-Z0-9_-]` 之外的字节写成 `_XX`（大写十六进制），
//!    `_` 原样留着。`srv:fs/read` → `srv_3Afs_2Fread`，
//!    `srv:get_time` → `srv_3Aget_time`——名字在日志和请求体里仍然认得出来。
//! 2. 严格档：`_` 也转义成 `_5F`。只有可读档还原不回原名时才用
//!    （名字里本来就带 `_3A` 这种「长得像转义」的片段）。
//!
//! **可逆性不是靠推理保证的，是靠 `to_wire` 自己验的**：产出前先 `from_wire`
//! 一遍，还原不回原名就退到严格档，而严格档的输出里 `_` 只可能是转义标记，
//! 必然可逆。同一个解码器吃两档，不需要标记用了哪档。
//!
//! 这个自校验顺带保证了**单射**：两个不同的名字若映到同一个 wire 名，
//! `from_wire` 不可能同时还原出两者，至少一个会在自校验时被打回严格档。

use std::sync::Arc;

const HEX: &[u8; 16] = b"0123456789ABCDEF";

pub fn to_wire(name: &str) -> String {
    let readable = escape(name, false);
    if *from_wire(&readable) == *name {
        return readable;
    }
    escape(name, true)
}

pub fn from_wire(wire: &str) -> Arc<str> {
    let src = wire.as_bytes();
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'_'
            && i + 2 < src.len()
            && let (Some(hi), Some(lo)) = (nibble(src[i + 1]), nibble(src[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(src[i]);
            i += 1;
        }
    }
    // 还原不出合法 UTF-8 只可能是对方乱改了名字：原样退回 wire 名，
    // 让「工具找不到」在 router 那里报出来，而不是在这里 panic。
    String::from_utf8(out).map_or_else(|_| Arc::from(wire), Arc::from)
}

fn escape(name: &str, underscore_too: bool) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || (b == b'_' && !underscore_too) {
            out.push(b as char);
        } else {
            out.push('_');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    out
}

/// 只认大写十六进制：`escape` 只产大写，收窄取值范围能让 `get_ab` 这类
/// 未转义名字原样通过。
fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_wire_safe(s: &str) -> bool {
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    /// 常见名字走可读档：下划线原样留着，名字在请求体里认得出来。
    #[test]
    fn namespaced_name_stays_readable() {
        assert_eq!(to_wire("srv:fs/read"), "srv_3Afs_2Fread");
        assert_eq!(to_wire("srv:get_time"), "srv_3Aget_time");
        assert_eq!(to_wire("get_weather"), "get_weather");
        assert!(is_wire_safe(&to_wire("srv:get_stock_price")));
    }

    /// 名字里本来带「长得像转义」的片段 → 自校验发现还原不回去，退到严格档。
    #[test]
    fn ambiguous_name_falls_back_to_strict_escape() {
        assert_eq!(to_wire("a_3Ab"), "a_5F3Ab");
        assert_eq!(&*from_wire("a_5F3Ab"), "a_3Ab");
    }

    #[test]
    fn round_trip() {
        for name in [
            "srv:fs/read",
            "srv:get_time",
            "web:dom/query",
            "desk:shell/exec",
            "plain_name",
            "a_3Ab",
            "a_5F3Ab",
            "get_5Fweather",
            "a-b",
            "带中文的:工具/名",
            "",
        ] {
            let wire = to_wire(name);
            assert!(is_wire_safe(&wire), "转义结果超出字符集：{wire}");
            assert_eq!(&*from_wire(&wire), name, "往返不等：{name}");
        }
    }

    /// 没被转义过的朴素名，还原是恒等的。
    #[test]
    fn plain_names_pass_through_unchanged() {
        for name in ["get_weather", "get_time", "lookup_record", "a_bc"] {
            assert_eq!(&*from_wire(name), name);
        }
    }
}
