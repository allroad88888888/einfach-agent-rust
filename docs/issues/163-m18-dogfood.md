# 163 真机收官 + 文档清账 ← M18 终点

**里程碑** M18 · **依赖** [160](160-recover-limits-param.md) + [161](161-server-bin-limits-flags.md)
· **模型** 主会话前台 · **独测** 本条即验收 · **状态** 未开始

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
