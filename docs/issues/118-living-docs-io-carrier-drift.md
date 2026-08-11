# 118 活文档里的「IO 线程池」措辞已对不上形状

**里程碑** M13 收尾 · **依赖** 114 · **模型** sonnet · **独测** —（纯文档）

117 换了 IO 载体、114c 加了 wasm 分身，但几份**活文档**的措辞停在换之前。
**结论都还成立，形状描述错了**——这类错法最坏：读的人不会怀疑，会照着错的形状做判断。

## 具体哪几处

| 文件 | 现在写的 | 实际形状 |
|---|---|---|
| `docs/ADAPTER.md:232-248` | 时序图分「actor 线程 \| IO 线程」两栏 | 没有「IO 线程」这一栏了 |
| `docs/ARCHITECTURE.md:19` | 「在 IO 线程池上并发」 | 泵同线程的 `FuturesUnordered` |
| `docs/STATE-MODEL.md:327` | 同上 | 同上 |
| `docs/ORCHESTRATION.md:10` | 同上 | 同上 |
| `docs/MCP.md` | 「IO 线程」 | MCP 那条**确实还是线程**（`mcp_call.rs:86`，117 明确保留），这里可能是对的，逐条确认 |

**换之后的真实形状**：provider 调用是**泵同一条线程上**的 `FuturesUnordered`；
native 底下只有一条工作线程，职责窄到只剩「把阻塞 socket 的字节读成行」
（`io_stream/native.rs`）；**wasm 上一条线程都没有**（`io_stream/web.rs` 两个
`spawn_local`）。「并发是 IO 并发、回写串行」这个结论一个字没变。

## 顺带一起收的两处命名漂移

- **`ProviderRequest` → `Encoded`**：决策 16 的结论仍活在代码里
  （`agent-providers/src/lib.rs:63` 那段注释就写着决策 16），但承载者改名了。
  `docs/ROADMAP.md` 决策 16、`docs/issues/025:43`、`111:57`、`115:32/37/64` 都还写老名字。
  **只改名字与形状描述，不要动结论**——决策 16 的双理由（线程边界 / `check_drift` 快照）
  2026-08-11 刚补注过，第二个理由才是保留它的原因。
- `117-io-without-threads.md`：L68/L158 写 `runner.rs` 364 行（现 431）；
  L76/L82/L154 把 `heartbeat.rs` / `io_stream.rs` 当文件写，114c 之后两者都是**目录**；
  L154-157 的遗留「114 接 wasm 时各换一份实现」已经做完了，措辞停在待做。

## 不在本 issue 范围

- `docs/ADAPTER.md:46` 与 `:269` 断言 `Ingredients: Send` 是编译期挡住 adapter
  拿 store 句柄的机制，但 `agent-providers/src/lib.rs:42` 的 `Ingredients<'a>`
  **没有任何 `Send` bound**（全 crate 只有 `pub trait Provider: Send + Sync`）。
  这是**既有**漂移，跟 M13 无关，而且它牵扯的是一条安全性断言到底还成不成立
  ——**是代码该补 bound，还是文档该改说法，得先判断**，不是改措辞能了事的。另开。
- `093-vision-subagent-delegation.md` / `.zh-CN.md` 点名的实现文件已被 s5 全部删除，
  `README.md:385-387` 有覆盖横幅但这两个文件自身没有（对比 `IMAGES.md:3-6` 是有的）。
  属于 s5 的收尾，不是 M13 的。

## 验收

- 上表每一处都改到与代码对得上，**改完贴出对应代码行作为依据**，不要凭印象改。
- `MCP.md` 那条要**逐条确认**是不是真的还该说「线程」——它可能本来就是对的。
- 结论一个字不许动。只改形状描述与名字。
