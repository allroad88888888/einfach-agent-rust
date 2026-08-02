//! `f32` → 安全的 JSON 数。三家的 `temperature` 都走这条路——DeepSeek/GLM 原样
//!透传，Kimi 把值钳成 1.0 之后也用它格式化。
//!
//! 直转 `f64` 会带出 `0.699999988079071` 这种尾巴（写进请求体谁读谁以为是
//! bug），走一次最短往返表示；`NaN` / `Inf` 直接不发——JSON 表达不了，硬塞会让
//! 整个请求体不是合法 JSON。

use serde_json::Value;

pub fn finite(t: f32) -> Option<Value> {
    let n = t.to_string().parse::<f64>().ok()?;
    serde_json::Number::from_f64(n).map(Value::Number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_round_trip_no_float_tail() {
        let v = finite(0.7).unwrap();
        assert_eq!(v.to_string(), "0.7");
    }

    #[test]
    fn nan_and_inf_are_not_sendable() {
        assert_eq!(finite(f32::NAN), None);
        assert_eq!(finite(f32::INFINITY), None);
        assert_eq!(finite(f32::NEG_INFINITY), None);
    }

    #[test]
    fn ordinary_values_round_trip() {
        assert_eq!(finite(1.0).unwrap(), serde_json::json!(1.0));
        assert_eq!(finite(0.0).unwrap(), serde_json::json!(0.0));
    }
}
