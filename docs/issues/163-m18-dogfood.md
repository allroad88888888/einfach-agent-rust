# 163 真机收官 + 文档清账 ← M18 终点

**里程碑** M18 · **依赖** [160](160-recover-limits-param.md) + [161](161-server-bin-limits-flags.md)
· **模型** 主会话前台 · **独测** 本条即验收 · **状态** 完成（2026-08-13）

## 目标

真 provider 走一遍「上限调小 → 模型撞闸自纠 → 重启恢复后闸还在」，把这批的账清完。

**这批的意义就在第 3 条**：前两条是决策 20 早就验过的行为（029/M8 真机见过模型撞
`is_error` 自纠），第 3 条才是 M18 新买到的东西——**恢复后闸不悄悄退回 8**。

## 真机脚本（HTTP server 形态，DeepSeek）

1. `agent-server --max-children 2 --sessions-dir <dir>` 起进程 → `POST /sessions`
   → 首轮请求体里 `srv:agent/spawn` 的描述写的是 **2**（不是 8）。
2. 给一个明显需要多个子的任务 → 模型 spawn 第 3 个时拿到
   `TooManyChildren { max: 2 }` 的 `is_error` → **自己收敛**（改成分批或减少子数），
   整轮仍然答成功。留原始帧。
3. **本批的主验收**：`kill -9` → **用同一份参数**重开 → 恢复那个会话再跑一轮：
   - `srv:agent/spawn` 描述里仍是 **2**；
   - spawn 第 3 个仍被 `max: 2` 拒（不是 8）；
   - 请求体前缀与崩溃前逐字节一致（sha256 比对），缓存命中率不掉。
   > 160 之前这一条必红：恢复出来的会话闸退回 8，而描述里还是 2。
4. **不给 flag 的对照会话**：请求体与 M18 之前的二进制 sha256 相等（这批不该让
   默认部署变一个字节）。
5. CLI 侧同款抽验一条（`agent-cli --max-children 2`，`--session` 落盘 → 重进 →
   闸仍是 2），不必跑满三轮。

## 文档清账

- ROADMAP §一：补**决策 32**（159 的拍板记录，理由写足）；§四清掉「子 agent 上限
  的配置面」那条未决问题；§二现状补 M18 一段。
- `issues/README.md`：M18 进度回填。
- CLAUDE.md 当前状态：M18 完成。
- **ORCHESTRATION.md**：决策 20 那两个数字现在**是可配的**，补一句配置面在哪
  （今天那份文档读起来像是硬编码）。
- **INTEGRATION.md**：Java 网关 `ProcessBuilder` 的参数表里点一句这两个 flag
  （网关本身不改，只是让下一个人知道能加）。
- `agent-core/src/command/spawn.rs` 与 `restore.rs` 的字段文档：160 改过的措辞
  在这里核对一遍，别留下一句「载入后重调」却没说通道在哪。

## 注意

- providers.toml 是 gitignored 的真钱 key，**绝不入库**；真机探针单飞
  （WORKFLOW §四第 -2 条）。
- 收工验证**前台跑完再交报告**（WORKFLOW §四第 -1 条）。
- 每条验收都要留数字（描述里的数字、拒绝时的 `max`、sha256、命中率），回填进
  本文件实做记录。

## 实做记录（主会话前台，2026-08-13，server + CLI 两种形态，真 DeepSeek）

**七条全过。** provider = deepseek/deepseek-v4-flash。线级用本地 recorder 当 provider
（149/158 同款手法，配置里换成假 key，真 key 全程不进 scratchpad——已用脚本核过）。

### 线级（recorder，不花钱）

| # | 验收 | 结果 |
|---|---|---|
| ① | `--max-children 2` 的数字进模型看的描述 | `children=2` ✅ |
| ② | 部分覆盖不连坐（depth 未配） | `depth=3`（默认档）✅ |
| ③ | 完全不给 flag = 决策 20 默认档 | `depth=3 children=8` ✅ |
| ④ | **不给 flag 时请求体与 M18 之前逐字节相同** | sha256 `8e21f0d0…a0a7`，双方 **8131 字节**，相等 ✅ |

④ 的旧二进制是 `bb43c83`（M18 之前）在一次性 worktree 里现编的。**验真**：旧二进制
`strings` 里 `max-children` 出现 **0** 次、`--help` 里 0 次，新二进制 4 次——确实是
两个不同的二进制在跑同一句输入。

### 真机（DeepSeek，花真钱）

**⑤ 撞闸 + 自纠**（`--max-children 2`，一句话诱导「一次性发三个子」）：模型一次发出
3 个 spawn，前两个成功（`root/a1` TCP、`root/a2` HTTP 缓存），第三个被拒：

```
spawn 失败：每个 agent 最多 2 个活着的直接子 agent，你已经有 2 个。
等手上这些回来之后再拆，或者少拆几个。          is_error: true
```

撞的是配置的 **2**，不是默认档的 8。模型对 DNS 那条**重试了 3 次**才收敛，最终自己
回答 C 并整合 A/B，整轮 `Done`。彩蛋：它在回复里准确指出「前两个虽已 Done 但仍占用
名额」——`max_children` 数的确实是**活着的**子（`spawn.rs:44` 的既有语义），模型从
错误文案里把这层读出来了。

**⑥ 主验收：`kill -9` 后闸还在**（本批唯一买到的新东西）。崩溃前会话 107086 字节、
36 行；`kill -9` 无优雅退出；**同一份参数**重开 → `POST /sessions` 带同 id →
`{"outcome":"recovered"}`。恢复后再诱导三个子：

- **三次 spawn 全部被拒**，错误正文仍是「最多 **2** 个」；
- 子树仍只有 `root/a1`、`root/a2`（没长出新节点）；
- 模型再次自纠，整轮 `Done`（回复原话：「连第一个都发不出去」）。

> **160 之前这一条必红**：恢复出来的会话 limits 退回 `default()` 的 8，而 root 只有
> 2 个活子，三个 spawn 会**全部成功**、树里会多出三个节点。

缓存：恢复后那一跳 `prompt=4778 cached=4608` = **96.4%**（≥0.9），`drift=Clean`、
`reconcile=Match{predicted:4608, actual:4608}`——三层兜底零告警。

**⑦ 恢复后描述也是 2**（闸与描述必须同一组数）：同一个恢复会话经 recorder 抓请求体，
`depth=3 children=2`。⑥ 证的是闸那半、⑦ 证的是描述那半，两半都没退回默认档。

### CLI 侧抽验（验收 5）

`agent-cli --session <path> --max-children 2` 跑两次：

- 新建：横幅 `子 agent 上限=深度≤3 子数≤2（默认 3/8）`，请求体 `children=2`；
- 同一文件重进：`[会话已恢复] 接着第 1 轮继续`，横幅与请求体**仍是 2**。

六个请求体逐个核过，`children` 全是 2。

## 坑

1. **`agent-server` bin 必须给 `--private-capability-stdin`**，否则任何请求都是 401
   `private_access_denied`——中间件挂在整个 router 上，且 `expected` 为 `None` 时
   `matches()` 直接返回 false（M9 为 Java 托管设计，`private_capability.rs`）。
   照 `run.rs` 的校验生成 43 字符 `[A-Za-z0-9_-]`，请求带
   `x-agent-server-capability` header。
2. **`GET /events/poll` 返回 JSON `{"frames":[{"id":..,"event":{..}}]}`，不是 SSE 文本**。
   照 SSE 的 `data:` 前缀去解析会一条都拿不到（第一版 driver 因此空转 240s，而轮次
   其实早就跑完了——是解析错，不是超时）。
3. **线上工具名是编码过的**：`srv:agent/spawn` → `srv_3Aagent_2Fspawn`
   （`agent_providers::wire_name`）。按原名在请求体里找工具会找不到。
4. **`TokenUsage` 不是 journaled slot**，会话 jsonl 里没有它。缓存数字在事件流的
   `turn_guard` 帧里（`{"usage":{"prompt":..,"cached":..},"report":{...}}`）。
   ring 有容量上限，早期帧会被挤掉并留下一条 `gap` 帧。
