# 140 宿主声明 skills 收编（server 路 + 恢复）

**里程碑** M15 · **依赖** [139](139-skill-assembly-switch.md) · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-11）

## 目标

`capabilities.skills` 走同一条新路：宿主声明的 skills 进 `SkillRegistry` 当
**第二个数据源**（内存，文件夹之外），`srv:skill/index` / `srv:skill/read`
同样服务它们；崩溃恢复后仍可 read。skills 从此在 server 侧也不是专用通道，
只是「宿主给工具包喂数据」。

## 现状

- 064 把宿主 skills 接进了 `with_skills` / `skill_injection`
  （`agent-server/src/http/capabilities/assemble.rs`、`sessions.rs`）。
- `host_skills` core 状态已有恢复测试（`host_skills_indep_restore.rs`）——
  它们来自一次活的 HTTP 请求，重启后请求不在了，状态就是唯一来源。**保留**，
  定位从「注入的源头」变成「registry 重建的原料」。
- 已拍板（TOOLS.md §多来源）：server 形态推荐不开磁盘装载；宿主声明撞磁盘
  已装载的 id → 400。
- `SkillSource::Host` 的授权语义（工具可逆性映射）在 v1 失去用武之地——
  skill 携带工具整个砍了（决策 27）。

## 做什么

1. assemble：宿主声明展开成 `Skill { id, description, body }` 进 registry，
   与磁盘源合并（撞 id → 400，既有闸）。
2. 恢复路：`host_skills` 状态 → 重建 registry → read/index 可用。
3. **声明里带 `tools` 的 skill → 400**，文案写明 v1 不支持（作者在场的
   最早可报点，比静默忽略诚实——069 判据）。协议文档补一句。

## 验收

- 宿主声明一个 skill 创建会话：首轮索引含它的 id + description；
  read 取到正文**逐字节**。
- 杀进程重启恢复：read 仍取到同一份正文（重建自 `host_skills` 状态，
  执行计数断言索引工具没有重跑——前缀块也是恢复来的）。
- 声明带 `tools` 的 skill → 整份 400，会话不创建。
- 宿主声明的 id 撞磁盘已装载的 id → 400（若该部署开了磁盘源）。

## 注意

- **红线 3/4 域**：恢复后 read 取错正文不报错——独测 agent 重点打恢复
  roundtrip。
- `agent-wasm` 不在本条：浏览器装配不开 skill 档（`assemble.rs` 模块文档
  明说），M14 的页面声明工具走的是 host tools 路，与 skills 无关。
- 动 `capabilities` 校验注意 `--features ts`（协议面文档注释都算，
  WORKFLOW §四第 4 步）。

## 实做记录（2026-08-11）

- 「做什么」第 1、2 点（registry 数据源、恢复路）已被 139 顺带完成，本条核实通过不重做。
- 新增校验：`capabilities.skills[..].tools` 非空 → 整份 400（`SkillCarriesTools { id }`），
  判定在 skill id 校验之后、工具前缀检查之前；文案含 skill id、`capabilities.tools` 指引、
  决策 27 出处。空 `tools` 数组仍合法（边界测试钉住）。
- validate.rs 拆出 validate_tests.rs（377→219+233，当次改动顶破 300 当次拆）；顺带删了
  决策 27 之后再无构造点的 `Origin::Skill` 死变体。
- 「磁盘撞 id → 400」经 rg 查证**不适用**：server 只调 `from_host_skills`（空表起、只吃
  声明），从未调 `SkillRegistry::load`——没有合流就没有撞名面，与 HOST-CAPABILITIES §八一致。
- 协议面只动了 `CapabilitySkill` 的文档注释：ts 一致性先红、`gen_protocol_ts` 重生成后
  全绿，diff 仅 `CapabilitySkill.ts` 注释。HOST-CAPABILITIES.md 新增 §八之二。
- 独测 `host_skill_reject_indep.rs` 三条全绿（400 不留半个会话、索引行+read 逐字节、
  重启后索引逐字节不变仍可 read）。
