//! 038：消息级 system 注入探针。实验设计见
//! docs/issues/038-skill-injection-probe.md，实现在 exp::system_inject。
//!
//! 独立 bin + 独立结果文件（不进 cache-prefix.json）——这组实验回答的是一个
//! 单独的设计问题（039 的注入策略），跟前缀缓存的其余五组实验不是一件事。
//!
//! 探针只报告观测到的数字，**不下结论**——结论写进 probes/PROVIDERS.md 由人来判。

use probes_api::client::Ctx;
use probes_api::{caps, config, exp};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RESULT_PATH: &str = "../results/system-inject.json";

struct Args {
    only_provider: Option<String>,
    delay: Duration,
    nonce: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        only_provider: None,
        delay: Duration::from_millis(3000),
        // 时间戳当 nonce：每次运行都从冷缓存开始，可重跑。
        nonce: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| format!("{}", d.as_secs()))
            .unwrap_or_else(|_| "fixed".into()),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--provider" => a.only_provider = it.next(),
            "--delay-ms" => {
                if let Some(v) = it.next().and_then(|v| v.parse().ok()) {
                    a.delay = Duration::from_millis(v);
                }
            }
            "--nonce" => {
                if let Some(v) = it.next() {
                    a.nonce = v;
                }
            }
            "--help" | "-h" => {
                eprintln!(
                    "用法: cargo run --bin system_inject -- [--provider <name>] [--delay-ms n] [--nonce s]"
                );
                std::process::exit(0);
            }
            other => eprintln!("忽略未知参数: {other}"),
        }
    }
    a
}

fn main() {
    let args = parse_args();
    let root = match config::load() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("nonce={}  间隔={:?}", args.nonce, args.delay);

    // 读回已有结果再合并——`--provider X` 不该抹掉另外两家之前的观测。
    let mut report: Value = std::fs::read_to_string(RESULT_PATH)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));

    for (name, p) in &root.providers {
        if args.only_provider.as_ref().is_some_and(|o| o != name) {
            continue;
        }
        println!("\n━━━ {name}  model={}  key={} ━━━", p.model, p.key_status());
        let Some(key) = p.resolve_key() else {
            println!("  跳过：未配置 key");
            continue;
        };
        let mut ctx = Ctx {
            provider: name.clone(),
            model: p.model.clone(),
            url: caps::endpoint(&p.base_url),
            key,
            delay: args.delay,
            nonce: args.nonce.clone(),
            observations: Vec::new(),
        };
        exp::system_inject::run(&mut ctx);
        report[name] = json!({
            "nonce": args.nonce,
            "model": p.model,
            "observations": ctx.observations,
        });
    }

    if let Some(dir) = std::path::Path::new(RESULT_PATH).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(RESULT_PATH, serde_json::to_string_pretty(&report).unwrap()) {
        Ok(_) => println!("\n原始观测已写入 probes/results/system-inject.json"),
        Err(e) => eprintln!("\n写结果失败: {e}"),
    }
    println!("结论请人工判读后写进 probes/PROVIDERS.md，探针不负责下结论。");
}
