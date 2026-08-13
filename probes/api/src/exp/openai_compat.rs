//! **裸 OpenAI 请求能不能跨家通用**的探针（issue 174）。
//!
//! 问的不是「这家能不能用」，而是一个更窄的问题：
//!
//! > 一个**完全不知道对面是谁**的通用 adapter，发一份标准 OpenAI 形状的请求
//! > （没有 `thinking`、没有各家的 temperature 特例、没有任何 `caps::` 分支），
//! > 打到某个自称 OpenAI 兼容的 `base_url` 上，会发生什么？
//!
//! 这正是 [175](../../../docs/issues/175-openai-compat-decision.md) 里那个
//! 「加一个 `openai/` 目录放退化实现」的方案在 wire 上的样子。答案决定它成不成立。
//!
//! **为什么拿 DeepSeek 当第一个被探对象**：它的 `/v1` 就是标准 OpenAI 接口，
//! 而它同时又是本仓适配得最深的一家（`deepseek/encode.rs` 241 行、四种 `Adjustment`）。
//! 「同一个端点，特化请求 vs 裸请求」的对照因此格外干净——差异全部归因于请求本身，
//! 不掺服务端差异。
//!
//! 探针只记录观测，**不下结论**（见 probes/README.md）。结论人工判读后写进 issue 174。

use crate::http::Probe;
use serde_json::{Value, json};

/// 标准 OpenAI 端点：`<base>/v1/chat/completions`，`base` 里已有 `/v1` 就不重复加。
///
/// providers.toml 里三家写法不一：deepseek 是 `https://api.deepseek.com`（无 `/v1`）、
/// kimi 是 `.../v1`、glm 是 `.../api/paas/v4`。通用 adapter 将来也要面对这个问题
/// ——**这一行本身就是 175 要定的东西之一**（`base_url` 该不该由用户带全路径）。
pub fn openai_v1_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    // `PROBE_NO_V1=1`：不加 `/v1`，直接 `<base>/chat/completions`。
    // GLM 用得着——它的兼容端点是 `/api/paas/v4/chat/completions`，**没有 `/v1`**，
    // 硬加就 404。这个开关的存在本身就是一条观测结果。
    if std::env::var("PROBE_NO_V1").is_ok() || base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

/// **裸 OpenAI 请求**。这个函数是本探针的全部意义所在：它里面不许出现任何
/// `caps::` 调用、任何按 provider 分支的东西。一个通用 adapter 能发的就是这些字段。
pub fn vanilla(model: &str, tools: Vec<Value>, ask: &str) -> Value {
    let mut b = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": "You are a test assistant. Call tools exactly as asked." },
            { "role": "user", "content": ask }
        ],
        "max_tokens": 256,
        // 0.0 是 OpenAI 的合法值。**故意不问 caps::temperature**——
        // 通用 adapter 不知道 Kimi 只收 1.0，这里就是要看「不知道」的代价是什么。
        "temperature": 0.0,
        "stream": false
    });
    if !tools.is_empty() {
        b["tools"] = Value::Array(tools);
    }
    b
}

pub fn weather_tool() -> Value {
    json!({"type":"function","function":{
        "name":"get_weather","description":"Get the weather of a city",
        "parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}})
}

/// A：裸请求能不能拿到一条正常回复。
pub fn probe_vanilla_chat(p: &mut Probe) {
    let body = vanilla(&p.model, vec![], "Reply with exactly: PONG");
    let (status, resp) = p.post(&body);
    p.record(
        "A/vanilla-chat",
        "裸 OpenAI 请求（无 thinking / 无 temperature 特例）能否正常回复",
        json!({
            "status": status,
            "finish_reason": resp["choices"][0]["finish_reason"],
            "content": resp["choices"][0]["message"]["content"],
            "usage": resp["usage"],
            "error": resp["error"],
        }),
    );
}

/// B：**usage 里的缓存字段叫什么、有没有**。
///
/// 这是 174 列的第 4 条观测，也是最可能咬人的一条：一个通用 adapter 的
/// `CACHED_PATHS` 该填什么？如果一家什么都不给，`stream/usage.rs` 会把「没有」
/// 读成 0 还是读成「不知道」？读成 0 就是让 024 的三层兜底拿到**假绿**。
pub fn probe_usage_shape(p: &mut Probe) {
    let body = vanilla(&p.model, vec![], "Say OK.");
    let (_, resp) = p.post(&body);
    let usage = &resp["usage"];
    let keys: Vec<String> = usage
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    p.record(
        "B/usage-shape",
        "usage 的全部字段名 —— 通用 adapter 的 CACHED_PATHS 要照着填",
        json!({
            "usage_keys": keys,
            "usage": usage,
            "openai_standard_path": usage["prompt_tokens_details"]["cached_tokens"],
            "deepseek_path": usage["prompt_cache_hit_tokens"],
        }),
    );
}

/// C：**工具调用的分片形状**。历来是各家最爱跑偏的地方（174 列的第 3 条）。
pub fn probe_tool_calls(p: &mut Probe) {
    let body = vanilla(
        &p.model,
        vec![weather_tool()],
        "What is the weather in Paris? Use the tool.",
    );
    let (status, resp) = p.post(&body);
    p.record(
        "C/tool-calls-nonstream",
        "非流式的 tool_calls 形状",
        json!({
            "status": status,
            "finish_reason": resp["choices"][0]["finish_reason"],
            "tool_calls": resp["choices"][0]["message"]["tool_calls"],
        }),
    );

    let mut sb = vanilla(
        &p.model,
        vec![weather_tool()],
        "What is the weather in Paris? Use the tool.",
    );
    sb["stream"] = json!(true);
    sb["stream_options"] = json!({ "include_usage": true });
    let lines = p.stream(&sb, 60);
    p.record(
        "C/tool-calls-stream",
        "流式的 tool_calls 分片：index 怎么给、arguments 怎么拼、[DONE] 有没有",
        json!({
            "line_count": lines.len(),
            "lines": lines,
        }),
    );
}

/// D：**通用 adapter 猜错时的报错形状**。
///
/// 两发：模型名不存在、以及一个 OpenAI 有而这家不一定有的字段。
/// 通用 adapter 的 `classify` 要能从这里认出该归哪一类。
pub fn probe_error_shapes(p: &mut Probe) {
    let mut bad_model = vanilla("definitely-not-a-real-model-xyz", vec![], "hi");
    bad_model["max_tokens"] = json!(16);
    let (status, resp) = p.post(&bad_model);
    p.record(
        "D/bad-model",
        "模型名不存在时的状态码与错误体形状",
        json!({ "status": status, "body": resp }),
    );

    // OpenAI 的 `n`（要几条候选）。很多兼容实现不支持，看它是拒绝还是静默忽略——
    // **静默忽略比拒绝糟**：通用 adapter 会以为自己拿到了要的东西。
    let mut with_n = vanilla(&p.model, vec![], "Say OK.");
    with_n["n"] = json!(2);
    let (status_n, resp_n) = p.post(&with_n);
    p.record(
        "D/unsupported-field-n",
        "OpenAI 的 n=2：拒绝、还是静默按 1 处理",
        json!({
            "status": status_n,
            "choice_count": resp_n["choices"].as_array().map(|a| a.len()),
            "error": resp_n["error"],
        }),
    );
}

/// E：**同一端点上「特化请求 vs 裸请求」的缓存对照**。
///
/// 本仓的 DeepSeek adapter 会做四种妥协；通用 adapter 一种都不做。
/// 如果裸请求的缓存命中不比特化的差，那 175 的退化实现方案的代价就是可量化的小。
pub fn probe_cache_parity(p: &mut Probe) {
    // 同一份长 prompt 连打两次，第二次应当命中缓存。
    let long = "The quick brown fox jumps over the lazy dog. ".repeat(120);
    let ask = format!("{long}\n\nReply with exactly: ONE");
    let body = vanilla(&p.model, vec![], &ask);

    let (_, first) = p.post(&body);
    std::thread::sleep(std::time::Duration::from_secs(2));
    let (_, second) = p.post(&body);

    p.record(
        "E/cache-parity",
        "裸请求连打两次：第二次的缓存命中（不做任何前缀特化处理）",
        json!({
            "first_usage": first["usage"],
            "second_usage": second["usage"],
        }),
    );
}

/// F：**最小内核请求**——只发「每个 OpenAI 兼容实现都必须支持」的字段。
///
/// 这是本探针最关键的一发，它在 [175](../../../docs/issues/175-openai-compat-decision.md)
/// 的两个方案之间做裁决：
///
/// - E 组已经证明「发全套 OpenAI 字段」会在 Kimi 上 400（`temperature: 0.0` 被拒）。
/// - 如果**连 `temperature` 都不发**就能三家全过，那通用 adapter 的契约可以写成
///   「只发最小内核，把取值交给对面的默认」——**整类「合法值被这家拒绝」的问题
///   在结构上消失**，不需要任何 per-endpoint 的怪癖表（那玩意就是 `match provider`
///   换了个地方住，红线 12 的形状）。
/// - 如果连最小内核都有家挑刺，那就只能上怪癖表，175 的成本要重估。
pub fn probe_minimal_core(p: &mut Probe) {
    let body = json!({
        "model": p.model,
        "messages": [
            { "role": "system", "content": "You are a test assistant." },
            { "role": "user", "content": "Reply with exactly: PONG" }
        ],
        "max_tokens": 64,
        "stream": false
        // 注意这里【没有】temperature、没有 n、没有 stream_options、没有 top_p。
    });
    let (status, resp) = p.post(&body);
    p.record(
        "F/minimal-core",
        "只发最小内核（无 temperature/n/top_p）：能不能三家全过",
        json!({
            "status": status,
            "content": resp["choices"][0]["message"]["content"],
            "finish_reason": resp["choices"][0]["finish_reason"],
            "error": resp["error"],
            "usage": resp["usage"],
        }),
    );

    // 带工具的最小内核 —— 工具是必须发的，不能省。
    let mut with_tools = body.clone();
    with_tools["tools"] = json!([weather_tool()]);
    with_tools["messages"][1]["content"] = json!("What is the weather in Paris? Use the tool.");
    let (st, rt) = p.post(&with_tools);
    p.record(
        "F/minimal-core-tools",
        "最小内核 + tools：工具调用在三家都通吗",
        json!({
            "status": st,
            "finish_reason": rt["choices"][0]["finish_reason"],
            "tool_calls": rt["choices"][0]["message"]["tool_calls"],
            "error": rt["error"],
        }),
    );
}

