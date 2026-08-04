# einfach-agent

企业级 Agent 运行时。核心是一个**原子状态引擎**——agent 的全部状态活在一张依赖
图里，因此 **undo / redo / 崩溃恢复 / 审计回放是同一套机制的四个投影**，不是四个
功能。同一个核心库，四种形态，全部经真实世界验收：

| 形态 | 入口 | 能干什么 |
|---|---|---|
| CLI | `cargo run -p agent-cli -- --session s.jsonl` | 对话、模型调工具、`/undo`（prompt 级真回滚）、`/undo!` 越不可逆屏障、`kill -9` 重启续聊 |
| 浏览器 | `cargo run -p agent-server --example serve` + `pnpm --filter web dev` | SSE 流式、模型 spawn 子 agent 并行（帧带归属）、断开自动取消在飞 |
| 独立 server | `cargo run -p agent-server-bin -- --sessions-dir ./sessions` | 六端点 HTTP/SSE，企业网关挡在前面即可（无鉴权是设计，见决策 11） |
| 桌面 | `pnpm --filter desktop tauri build` | Tauri 内嵌同一个 server 库 + 同一套 web 前端（逐文件同哈希） |

企业内嵌参考：`examples/java-gateway/`（Spring WebFlux 反向代理，拷走改）。

## 起步

```bash
cp providers.example.toml providers.toml   # 填 DeepSeek / Kimi / GLM 任一家的 key
cargo test --workspace                     # 954 个测试
cargo run -p agent-cli
```

## 三个不寻常之处

1. **状态即真理**：完整状态 = 所有 primitive atom 的值。恢复 = 快照 + redo，
   字面上同一个函数（`apply_next`）。被 `/undo` 撤销的轮次在模型的记忆里不存在。
2. **模型差异关在 adapter 里**：core 一条模型相关判断都没有（红线 12，编译期
   保障）。core 说意图，adapter 做不到就报 `Adjustment`——可见、可审计。
   三家（DeepSeek/Kimi/GLM）的实测差异档案在 `probes/PROVIDERS.md`。
3. **缓存是钱**：前缀缓存命中差价最高 120 倍。三层兜底（发前字节比对 / 收后
   对账 / 滚动窗口）让缓存失效从「月底看账单」变成「当轮看告警」。

## 文档

新会话/新人从 [CLAUDE.md](CLAUDE.md) 的文档地图进。要点：
[ROADMAP](docs/ROADMAP.md)（20 条已拍板决策 + 未决问题）·
[INVARIANTS](docs/INVARIANTS.md)（12 条红线，违反不报错所以才是红线）·
[STATE-MODEL](docs/STATE-MODEL.md) · [ADAPTER](docs/ADAPTER.md) ·
[issues/](docs/issues/README.md)（37 个已完成 issue，每个带实做与合并记录）。

红线由 `scripts/check-invariants.sh` 在编辑钩子与 CI 上强制执行。

## 状态

M1–M5 全部完成并真实验收（2026-08）。M6（MCP 接入）与插队的 M7（子 agent 可观测，
[ROADMAP §二](docs/ROADMAP.md)）进行中：M7 四个 issue（core 派生读 / CLI `/agents` /
SSE 快照事件 / web 活树面板）代码均已交、typecheck + build 前台跑绿，**真浏览器终验**
（spawn 子 agent 实时长树、`/undo` 回退、断线重连恢复）待做。未排期（等真实使用
反馈）：前后端工具闭环收尾、多租户。上游血缘：状态引擎 fork 自
[einfach](https://github.com/allroad88888888/einfach) 的 Rust 原子引擎，独立演进。
