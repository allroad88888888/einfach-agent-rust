# 159 决策：子 agent 上限的配置面开在哪一层

**里程碑** M18 · **依赖** — · **模型** **opus** · **独测** 决策类 · **状态** 完成（2026-08-12 拍板 = 决策 32）

## 要决什么

`AgentLimits { max_depth: 3, max_children: 8 }`（决策 20 的成本兜底）**在代码里
一直是可配的**——`Session::set_agent_limits` 有、`ToolTableSpec::Full { spawn_limits }`
有、`ToolTable::with_spawn(limits)` 有。**缺的只是一个运行时入口**：四个生产装配点
全部写死 `AgentLimits::default()`，没有任何 flag / 环境变量 / 配置字段能改它。

要定的是**配置面开在哪一层**，以及随之而来的恢复语义。不定就动手的话，最容易
顺手做成「`POST /sessions` 加个字段」，而那条路会撞上下面第 2 条。

## 现状（勘查结论，动手前自己再核一遍）

**写死的四处**（生产代码，`builtin_switch.rs:121` 那处是测试夹具不算）：

| 位置 | 说明 |
|---|---|
| `agent-server-bin/src/run.rs:51` | 独立 server bin，`ToolTableSpec::Full` |
| `agent-server/examples/serve.rs:53` | 示例，同上 |
| `agent-server/src/registry/spec.rs:186` | `SessionTemplate` 默认档 |
| `agent-core/src/command/session.rs:97` | `Session::new` 兜底 |

`agent-cli/src/main.rs:186` 只是 `.with_spawn(session.agent_limits())` **读**，不设置。

**好消息：对齐接缝已经在了。** 两侧数字必须一致（真正拦人的是 `Session::spawn_child`
的两道闸，写进工具描述给模型看的是 `ToolTable::with_spawn`），这个耦合 034 已经做成
一次函数调用——`ToolTableSpec::spawn_limits()` 读口 + `actor/body.rs:86` 建会话时
`set_agent_limits`。**server 侧只要让 `ServerConfig` 里那个值不再写死，全链自动通。**

**坏消息：恢复路径是漏的，今天被 default ≡ default 掩盖着。**

- `agent-core/src/command/restore.rs:124-128` 恢复时**硬写** `AgentLimits::default()`，
  注释说「和 `history_cap` 一样是载入后重调」；
- 可 `agent_runtime::recover(store, agent, history_cap, on_unknown_key)` 给了
  `history_cap` 入参，**没给 `limits`**——宿主想「载入后重调」也没有通道；
- 于是 `actor/body.rs:80-88` 那句「恢复出来的会话带着它自己持久化过的配置，不被
  这一刻的服务端配置悄悄改写」对 `history_cap` 成立，**对 `limits` 是假的**。

今天配置值恒等于 default，两边永远相等，所以看不出来。**一旦上限可配，第一次重启
就显形**：`--max-children 16` 建的会话恢复后闸悄悄退回 8，而工具描述里还写着 16——
正是 `registry/spec.rs:130` 那段文档最怕的两侧失配，且**静默**（模型按 16 规划，
撞上 8 那道闸，得到一条它无法从描述里预见的拒绝）。

**wasm 宿主不在范围内**：`agent-wasm` 里 `with_spawn` 零命中，那个形态根本没开
子 agent 能力。**桌面同理不单开面**（内嵌库，走装配默认）。

## 三个候选

### A. 进程级启动参数（**推荐**）

`agent-server-bin` / `agent-cli` 各加两个 flag（`--max-agent-depth` /
`--max-children`，带环境变量兜底），落进 `ToolTableSpec::Full { spawn_limits }`
与 `Session`。协议面一个字节不改。

- ✅ 与「limits 是**配置**不是状态」的既有拍板（`spawn.rs:47-51` 字段文档 +
  `restore.rs:124` 注释）一致，不推翻任何东西。
- ✅ 恢复天然一致：同一份启动参数重启，恢复出来的会话拿到同一组数——**前提是
  补上 `recover` 的 `limits` 入参**（[160](160-recover-limits-param.md)）。
- ✅ 部署方自负成本，符合决策 20「结构性硬限是成本兜底」的定位。
- ❌ 同一进程里所有会话共用一组数。

### B. per-session，进 `POST /sessions` 协议面

`capabilities` 里加 `spawn_limits: {max_depth, max_children}`。

- ❌ **立刻撞持久化**：per-session 的值必须跨恢复活下来，否则就是 A 的漏洞放大版
  （每个会话一组数，恢复后全变 default）。要它活下来只有两条路，都不好：
  进 store（见 C），或让宿主恢复时再说一遍——可宿主怎么知道该说哪组？除非把它
  记在会话文件旁边，那就是变相进 store。
- ❌ 决策 20 的定位是**部署方的成本兜底**。让建会话的客户端自己填上限，等于兜底
  由被兜的那一方决定。
- 除非有「同一部署里不同租户要不同上限」的真实用户——**今天没有**（多租户明确
  未排期，ROADMAP §二「没做的」）。

### C. 进 store 当 `Slot`（照 073 `HostTools` / 154 `HostPrefix`）

- ❌ 073/154 那条路是为**内容**设计的：声明的东西要进 prompt、要能回放、要能 undo。
  limits **不进 prompt**（它只影响 spawn 的闸 + 工具描述里那两个数字，而描述每次
  装配现生成），也**不能进 undo**——`spawn.rs:91` 已经拍过：调小不追溯，
  「撤回一次上限变更」只会让一批已存在的子 agent 变成非法。
- 一个不进 undo 的 slot 是半吊子，会把「journaled = 可撤」这条给模糊掉。

## 要给出的产出

1. 选定 A / B / C（推荐 A，否决理由若不认同要写清替代判据）。
2. **下限**：`max_children = 0` / `max_depth = 0` 合法吗？倾向**下限钉 1**——
   「关掉 spawn」有 076 的 `disable_builtin` 这条现成的路，工具描述里写「最多 0 个」
   是个坏形状（工具在表里却结构性不可用）。上限不设死限（部署方自负成本）。
3. **解析失败怎么办**：跟随 `server-bin/src/cli.rs` 既有取向（`--port` 解析不出来
   静默 `None` 交下游报错，见那个文件的 `unparseable_port_is_silently_none_not_a_panic`
   测试），还是这两个 flag 值得当场拒绝启动。两种都要给理由，别默认跟随。
4. 拍完写进 ROADMAP §一 成**决策 32**，并清掉 §四对应那条未决问题。

## 注意

- 决策类：一律 opus，必须有人拍板（WORKFLOW §二第三档 + §五）。
- **红线 12**：limits 是数字**参数**不是分支，`spawn.rs:17` 已经点名过这一点——
  不管选哪个候选，都不许在 core 里长出「按部署形态分支」的东西。
- 恢复失配那条（现状第二段）**不管选 A/B/C 都要修**，所以 160 不等这个决策，
  可并行开工。

## 拍板记录（2026-08-12，= 决策 32，全文在 ROADMAP §一）

**选 A（进程级启动参数），校验取严。**

1. **配置面 = 启动参数**，协议面零改动。B（per-session 进协议）与 C（进 store 当
   `Slot`）的否决理由照上文，拍板时没有新增反驳——B 的死结是「per-session 的值
   必须跨恢复活下来，而 limits 进不了 store」，C 的死结是「不进 undo 的 slot 是
   半吊子，而 `spawn.rs:91` 早就拍过上限变更不可撤」。
2. **产出第 2 条（下限）**：钉 1。关掉 spawn 有 076 的 `disable_builtin` 这条现成
   的路，工具留在表里却写「最多 0 个」是坏形状。上限不设死限（部署方自负成本）。
3. **产出第 3 条（解析失败）**：**拒绝启动**。
   拍板时把这条写成「偏离 `--port` 的既有先例」，**161 实做时更正**：本仓
   **两种取向都有先例**，判据是**有没有下游替它报错**——
   - `--port` 静默退 `None`，因为 `default_bind_addr` 到真要用时会报「配置错了」
     （`cli.rs` 的 `unparseable_port_is_silently_none_not_a_panic` 记着这层取舍）；
   - `AGENT_BIND` 配成非法 IP 直接硬失败（`agent_server::bind::BindConfigError`
     的文档：「用户显式设了这个变量就是想覆盖默认值，把打错的字符串当成没设，
     是那种配置错了却看起来在正常运行的坑」）。

   上限**没有下游**，所以归后一类。这不是偏离先例，是**跟随更贴切的那个先例**。

**开工勘查的意外收获**（已单开 160 并完成）：恢复路径的静默失配。`restore` 硬写
`AgentLimits::default()` 且 `recover` 没有入参，`restore.rs` 那句「宿主要非默认值，
恢复后调 `set_agent_limits`」名存实亡。**这个洞今天不显形只因为配置值恒等于默认
值**——正是本决策要打破的那个恒等。顺带查实：`actor/body.rs` 里「恢复出来的会话
带着它自己持久化过的配置」这句话对 `limits` 和 `history_cap` **都**不成立（两者都
不持久化，都靠宿主再说一遍），注释已一并更正。
