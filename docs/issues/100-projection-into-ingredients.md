# 100 投影接进料单与 `encode`

**里程碑** M12 · **依赖** [099](099-send-plan.md) · **模型** sonnet · **独立测试 agent** 是 · **状态** 完成

## 目标

让 099 的投影真的生效：adapter 取料时拿到的是**投影后的历史**，不是完整历史。

这一条走通之后，B 支（第 2 档）与 C 支（第 3/4 档）就能完全并行开工。

## 做什么

接线，不做判断。投影在 core（它是纯函数，core 有权算），摆盘在 adapter
（决策 18 的第三分，不变）。adapter **不知道**有压缩这回事——它只是拿到一份历史。

## 验收

- 清掉 3 条工具返回后，`encode` 出的请求体里这 3 条**不出现**
- 前缀镜像的 History 段 hash 随之改变，Tools / System 两段**不变**
  （压缩只动 History，动了另外两段说明接错了地方）
- **三家（DeepSeek / Kimi / GLM）都生效**，且各自的 `encode` 里没有为压缩新增任何分支
- 同一份 `(历史, SendPlan)` 连续 `encode` 两次，请求体逐字节相同
- 空 `SendPlan` 时，`encode` 输出与本 issue 落地**之前**逐字节相同——
  不用这个功能就逐字节不变（065、067 都是这么做的）

最后一条最重要：它保证已有的一批 golden 测试不用改，也证明接线没有引入任何
无条件的形状变化。

## 注意

- **红线 11**——`encode` 的确定性是已经锁死的（023 有 `encode_determinism` 测试），
  接线不能把它破坏
- **红线 12**——投影结果进 adapter，但 adapter 不许因为「这段是摘要」而走不同的路。
  摘要在它眼里就是一条普通消息
- `wire::prefix::concat` 那条注释（逐条拼接不套数组）在这里仍然生效，别顺手改成套数组

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

### 本条钉死的接口

```rust
impl Session {
    pub fn send_plan_of(&self, agent: &AgentId) -> SendPlan;          // 默认 pristine
    pub fn replace_send_plan(&mut self, agent: &AgentId, plan: SendPlan);  // 低层 setter
}
```

`replace_send_plan` 刻意做成**不含策略的整体替换**——谁该被清、什么时候清是
101/102 的事。这样 100 只负责「让投影真的生效」，测试也有办法把非空计划塞进去验。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-core/src/value/send_plan_codec.rs`（新建） | 68 | `SendPlan ↔ AgentValue::Json` 编解码 |
| `agent-core/src/command/send_plan.rs`（新建） | 141 | `send_plan_of` / `replace_send_plan`，进 undo log |
| `agent-core/src/graph/slot.rs` | 285（+28） | `Slot::SendPlan` 变体 + 默认值 + 进 `Slot::ALL`（15→16） |
| `agent-core/src/graph/visibility.rs` | 189（+5） | `Slot::SendPlan` 归 `Private` |
| `agent-runtime/src/provider_call.rs` | 238（+13） | 取料处接上 `project` |
| 6 个既有测试 | 机械 | 槽位计数触发线 15→16 + 一个穷举 `match` 分支 |
| `agent-providers/tests/it/send_plan_{clearing,three_providers,pristine_is_invisible}.rs`（新建） | 142/161/99 | 独测：组合级与三家横向 |
| `agent-core/tests/it/send_plan_of_session_default.rs`（新建） | 81 | 独测：命令层读写与 undo 契约 |
| `agent-runtime/tests/it/send_plan_wiring_{clears_tool_results,undo_restores_bytes}.rs`（新建） | 94/84 | 独测：真实 `run_turn` + 录制服务器的端到端 |

### 设计判断（实现 agent 裁决，主会话复核后收）

1. **加 `Slot` 变体，不加 `AgentValue` 变体。** `atom_value.rs` 模块文档自己写着
   「026 一次定死，Slot 表可以随里程碑长，值的形状不再动」；026 之后新增的六个
   槽位（`ToolsAllowed`/`SkillsActive`/`HostTools`/`HostSkills`/`DisabledBuiltins`/
   `ExecutionProfile`）无一开新变体。走 `AgentValue::Json` + 专用 codec 模块，
   照 `str_set.rs` / `host_tools.rs` 的先例。**`atom_value.rs` 零 diff（已复核）。**
2. **可见性 `Private`。** `SendPlan` 是这个 agent 自己的发送侧账本，跟
   `PrevPrefix` / `ToolSlots` 同类，不是父要给子的上下文（`Upward`）也不是父要等
   子的结果（`Downward`）。照 `visibility.rs` 自己的规矩「开放一个方向要有理由，
   封闭不需要」，095/099/100 都没给出理由，所以默认关闭。
3. **默认值是 pristine 的编码结果，不是 `Null`。** 所有状态（含 pristine）走同一条
   codec 路径，读的人不必区分「从没写过」和「写了个空计划」——它们就是同一个值。
   这也是「不用这个功能就逐字节不变」的字面机制。
4. **`replace_send_plan` 不带守卫**（不查 `in_session` / `is_live`）。带守卫的
   setter（`skill.rs` 的 `activate_skill`）之所以查，是因为**接受/拒绝本身就是那条
   命令的业务语义**；本函数没有这种语义，校验存活是 101/102 调它之前的事。

### 那六个既有测试为什么可以改

主会话专门核过：改的全是**槽位计数触发线**（15→16）加一个穷举 `match` 分支
——那种测试存在的目的就是逼加槽位的人停下来想一下，改它是应尽的记账（093 加
`ExecutionProfile` 时做过同样的事）。

**encode 的 golden 一个没动**（`agent-providers/tests/it/` 只改了 `main.rs` 的 mod 行
和 support 助手），那才是「不用这个功能逐字节不变」真正要守的东西。

### 变异检验（主会话做）

注入最难发现的那类接线 bug——**投影接上了，但永远用 pristine 计划**
（`send_plan_of` 的结果被丢掉）：

```
send_plan_wiring_clears_tool_results::clearing_three_tool_results_...   FAILED
```

**⚠️ 只有这一个测试抓住。** 另外两处在这个变异下都是绿的，而且是有道理的绿：

- `send_plan_wiring_undo_restores_bytes` **空洞地通过**——计划根本没被读，
  「清除前」和「undo 后」都是未投影的历史，自然相等。它不是接线的独立锁。
- `agent-providers` 那 5 个测的是 `project` + `encode` 的组合，不经过接线，
  绿是正确的分层。

**结论：「取料处真的读了 `SendPlan`」这条性质目前由单点保护。** 101/102 落地后
会自然多出几条端到端断言；在那之前，动 `provider_call.rs` 取料那几行的人要知道
自己只有一张网。

### 命令输出

```
$ cargo test --workspace
1604 passed; 0 failed

$ bash scripts/check-invariants.sh --all
exit 0；17 条行数提示全是存量文件

$ cargo clippy -p agent-core -p agent-providers --all-targets -- -D warnings
干净

$ cargo clippy -p agent-runtime --all-targets -- -D warnings
5 个存量错误，与 baseline 逐条相同，零新增
```

## 事后发现的一个 bug（104 落地时发现，2026-08-10）

**本条落地时漏了一步，会让任何用过压缩的会话重启后打不开。**

`replace_send_plan` 落 undo entry 用的是新 label `"replace_send_plan"`，但
`command/meta.rs` 的 `KNOWN_LABELS` 是个**封闭的编译期常量集**，本条没往里注册。
后果链条（`agent-runtime/src/persist/recover.rs:72`）：

```
用过压缩的会话 → 落盘 → 重启加载 → known_label() 返回 None
                → RecoverError::UnknownLabel → recover 硬失败
```

**为什么本条落地时 1604 个测试全绿也没抓到**：那时没有任何生产路径调
`replace_send_plan`，而调它的测试全停在内存里，**没有一条走完「落盘 → 重启 → 恢复」**。

**为什么 `meta.rs` 自己的测试永远抓不到**：

```rust
fn every_known_label_maps_back_to_itself() {
    for label in KNOWN_LABELS { assert_eq!(known_label(label), Some(*label)); }
}
```

它遍历的是 `KNOWN_LABELS` 自己——**少一项照样绿**。集合里没有的标签它根本看不见。

### 修法

- [104](104-boundary-command.md) 的实现 agent 补上了缺失的那一项
- 主会话补了回归测试
  `agent-runtime/tests/it/jsonl_restart_after_compaction_command.rs`（168 行，2 条）：
  压缩命令 → 落盘 → 新进程 → `recover` 成功**且压缩状态还在**。
  已验证：把那一项从 `KNOWN_LABELS` 拿掉，**这两条红、`meta.rs` 自己那两条照样绿**

### 留给后面的规矩

**M12 之后每加一条会落 entry 的命令，照 `jsonl_restart_after_compaction_command.rs`
的形状加一条重启回归。** 光靠 `meta.rs` 的自测和内存里的命令测试，这类 bug
一次都抓不到——它只在真的重启那天浮出来。


## 后续：那句 `summary_text: None` 后来成了一个 bug

本条落地时 `project(&history, &plan, None)` 是**正确**的——摘要还不存在，
issue 里也明确禁止了「自己发明一个摘要仓库」。

但 [107](107-summary-writeback.md) 把 `summary_text` 做出来之后，这一行就该跟上，
而 100/107/108 三条各自的范围都没盖住这根线。后果是**第 3 档整条路哑火**：
状态全对，发出去的字节一个都没压。详见
[108 §独测抓到的一个真 bug](108-tier-ladder.md)。

**这不是「当时应该预见」**——当时预见了就是超范围。真正的教训在验收那一侧：
**「状态写对了」和「发出去的字节对了」是两件事，验收要断言后者。**
