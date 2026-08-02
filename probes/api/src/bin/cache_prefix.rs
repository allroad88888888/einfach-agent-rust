//! 前缀缓存探针。实验清单见 probes/README.md。
//!
//! 探针只报告观测到的数字，**不下结论**——结论写进 probes/results/ 由人来判。

use probes_api::client::Ctx;
use probes_api::{caps, config, exp};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RESULT_PATH: &str = "../results/cache-prefix.json";

struct Args {
    only_provider: Option<String>,
    only_exp: Vec<String>,
    delay: Duration,
    nonce: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        only_provider: None,
        only_exp: Vec::new(),
        delay: Duration::from_millis(3000),
        // 默认用时间戳当 nonce：每次运行都从冷缓存开始，跑第二遍不会全命中。
        nonce: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| format!("{}", d.as_secs()))
            .unwrap_or_else(|_| "fixed".into()),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--provider" => a.only_provider = it.next(),
            "--exp" => a.only_exp.extend(it.next().unwrap_or_default().split(',').map(String::from)),
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
                eprintln!("用法: cargo run -- [--provider <name>] [--exp a,b] [--delay-ms n] [--nonce s]");
                eprintln!("实验:");
                for e in exp::ALL {
                    eprintln!("  {:<14} {}", e.id, e.title);
                }
                std::process::exit(0);
            }
            other => eprintln!("忽略未知参数: {other}"),
        }
    }
    a
}

fn selected(args: &Args) -> Vec<&'static exp::Experiment> {
    if args.only_exp.is_empty() {
        return exp::ALL.iter().collect();
    }
    args.only_exp
        .iter()
        .filter_map(|id| match exp::lookup(id) {
            Some(e) => Some(e),
            None => {
                eprintln!("未知实验: {id}");
                None
            }
        })
        .collect()
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
    let plan = selected(&args);
    println!("nonce={}  间隔={:?}", args.nonce, args.delay);
    println!("实验: {}", plan.iter().map(|e| e.id).collect::<Vec<_>>().join(", "));

    // 读回已有结果再合并 —— `--provider X` 不该抹掉另外两家的观测，
    // 而冷缓存那一轮的数据是不可复现的。
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
        for e in &plan {
            (e.run)(&mut ctx);
        }
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
        Ok(_) => println!("\n原始观测已写入 probes/results/cache-prefix.json"),
        Err(e) => eprintln!("\n写结果失败: {e}"),
    }
    println!("结论请人工判读后写进 probes/results/，探针不负责下结论。");
}
