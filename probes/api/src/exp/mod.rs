//! 实验注册表。一个实验一个文件。

pub mod granularity;
pub mod invalidation;
pub mod mutation;
pub mod sharing;
// 038 消息级 system 注入：签名兼容 ALL 的 `fn(&mut Ctx)`，但故意不注册进去——
// 它有自己的独立结果文件（system-inject.json，见 bin/system_inject.rs），
// 混进 cache_prefix 的默认全跑会把它的花费也捎带上，且写错文件。
pub mod system_inject;
pub mod thinking;

use crate::client::Ctx;

pub struct Experiment {
    pub id: &'static str,
    pub title: &'static str,
    pub run: fn(&mut Ctx),
}

pub const ALL: &[Experiment] = &[
    Experiment {
        id: "invalidation",
        title: "E0-E3 改动请求某处，前缀还认不认",
        run: invalidation::run,
    },
    Experiment {
        id: "granularity",
        title: "E4 缓存收益 vs 上下文规模",
        run: granularity::run,
    },
    Experiment {
        id: "sharing",
        title: "E5 子 agent 共享前缀",
        run: sharing::run,
    },
    Experiment {
        id: "mutation",
        title: "E6 末尾追加 vs 中间改写",
        run: mutation::run,
    },
    Experiment {
        id: "thinking",
        title: "E7 thinking.type 是否进前缀",
        run: thinking::run,
    },
];

pub fn lookup(id: &str) -> Option<&'static Experiment> {
    ALL.iter().find(|e| e.id == id)
}
