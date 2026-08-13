//! 174：裸 OpenAI 请求跨家通用性探针。**实验设计见 `exp::openai_compat`**，
//! 这里只是驱动：读配置、按 provider 循环、把观测合并写进结果文件。
//!
//! 独立 bin + 独立结果文件（openai-compat.json），照 038/`system_inject` 与 E10 的
//! 先例——它回答的是「通用 adapter 成不成立」这个设计问题，跟前缀缓存那五组不是
//! 一件事，混进默认全跑会捎带花费且写错文件。

use probes_api::config;
use probes_api::exp::openai_compat as oc;
use probes_api::http::Probe;
use serde_json::{Value, json};

const RESULT_PATH: &str = "../results/openai-compat.json";

fn main() {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("读 providers.toml 失败：{e}");
            std::process::exit(1);
        }
    };

    // 默认只探 deepseek —— 它的 /v1 是标准 OpenAI 接口，且它是本仓适配最深的一家，
    // 「同端点、特化 vs 裸」的对照最干净。`PROBE_PROVIDERS=kimi,glm` 可以扩。
    let want: Vec<String> = std::env::var("PROBE_PROVIDERS")
        .unwrap_or_else(|_| "deepseek".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // **合并写入，不整份覆盖。** 这个探针本来就设计成可以按 provider 分开跑
    // （`PROBE_PROVIDERS=kimi`），整份覆盖会让上一跑的观测悄悄消失——第一次用它
    // 就踩了：先跑 deepseek、再跑 kimi,glm，deepseek 那份直接没了。
    let mut report: Value = std::fs::read_to_string(RESULT_PATH)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    for name in &want {
        let Some(prov) = cfg.providers.get(name) else {
            eprintln!("providers.toml 里没有 [{name}]，跳过");
            continue;
        };
        let Some(key) = prov.resolve_key() else {
            eprintln!("{name}: key {}，跳过", prov.key_status());
            continue;
        };
        // **显式走 `/v1`**，不用 `caps::endpoint`。那个 helper 直接把 base_url 拼上
        // `/chat/completions`，而 providers.toml 里 deepseek 的 base_url 没有 `/v1`
        // ——本探针要打的恰恰是「标准 OpenAI 路径」，路径本身就是被探对象的一部分。
        let url = oc::openai_v1_endpoint(&prov.base_url);
        println!("\n=== {name}（裸 OpenAI 请求）===");
        println!("  endpoint: {url}");
        let mut p = Probe::new(name.clone(), prov.model.clone(), url, key);
        oc::probe_vanilla_chat(&mut p);
        oc::probe_usage_shape(&mut p);
        oc::probe_tool_calls(&mut p);
        oc::probe_error_shapes(&mut p);
        oc::probe_cache_parity(&mut p);
        // F 组放最后：它是 175 两个方案之间的裁决实验，前面几组的观测是它的背景。
        oc::probe_minimal_core(&mut p);
        report[name] = Value::Array(p.out);
    }

    if let Some(d) = std::path::Path::new(RESULT_PATH).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(RESULT_PATH, serde_json::to_string_pretty(&report).unwrap());
    println!("\n原始观测已写入 probes/results/openai-compat.json");
}
