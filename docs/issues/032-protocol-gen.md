# 032 `packages/protocol`：TS 类型从 Rust 生成

**里程碑** M3 · **依赖** 031 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

ARCHITECTURE §协议类型落地：线上协议**不存在两份手写副本**。TS 侧的
`SessionEvent`/`Command` 等全部由 Rust 生成，生成物与源不一致则测试失败——
决策 2（monorepo 双 workspace）的唯一存在理由在此兑现。

## 做什么

1. **pnpm workspace 起步**：根 `package.json` + `pnpm-workspace.yaml`
   （`packages/*`、`apps/*`），`packages/protocol/`（package.json + tsconfig，
   `typescript` devDep，脚本 `typecheck` = `tsc --noEmit`）
2. **ts-rs 接入**：协议面类型加 `#[derive(TS)]` + `#[ts(export)]`——
   `SessionEvent`/`UndoOutcome`/`Command`/`Granularity` 及其载荷可达的全部类型
   （`AgentId` 导出为 string 之类的映射自定并写明）。agent-core 里被引用的类型
   用 **feature 门**（`ts` feature）挂 derive，默认关——核心 crate 不为代码生成
   背常驻依赖
3. **生成与一致性**：`crates/agent-server` 一个 `#[cfg(feature = "ts")]` 测试
   导出到 `packages/protocol/src/generated/`；**一致性检查不依赖 git**（仓库
   尚无提交）：测试先导出到临时目录，与 `generated/` 逐文件字节比较，不一致
   即失败并打印差异文件名——「忘了重新生成」在 cargo test 层面就红
4. **serde↔TS 形状对齐的实检**：Rust 侧把每个 `SessionEvent` 变体的样本
   `serde_json` 序列化进 `packages/protocol/fixtures/events.json`（同样走
   一致性比较）；TS 侧一个 `fixtures.test.ts`（tsc 层面）：
   `import events from '../fixtures/events.json'` + `satisfies SessionEvent[]`
   ——邻接标签的 `type`/`data` 形状若与生成类型不合，`pnpm typecheck` 红

## 验收

- `cargo test -p agent-server --features ts` 全绿（含导出 + 一致性 + fixtures）
- 手改 `generated/` 任一文件一个字节 → 上述测试红且指出文件
- `pnpm install && pnpm -r typecheck` 全绿（fixtures satisfies 含内）
- `SessionEvent` 全部变体在 fixtures 里各至少一个样本（穷举 match 生成，
  编译器保证不漏）
- 生成物带「勿手改」头注释与再生成命令

## 注意

红线 12 不涉及（协议面在 server 层）。`node_modules/` 加 .gitignore。
生成的 TS 命名照 ts-rs 默认（类型名 PascalCase 保留），字段名跟随 serde
rename（snake_case）——TS 侧以线上真实形状为准，不为 TS 惯例改协议。

### 合并记录（主会话）

19 个生成 TS + fixtures 双格式（json 给一致性比对、as-const ts 给 satisfies——
JSON import 字面量加宽是 TS 语言行为，异议成立）。一致性测试三种篡改各自报清、
验证过真红。手动 export_all 替代 #[ts(export)] 的理由成立（自动机制插不进
临时目录比对工序）。默认 feature 零 ts-rs 依赖（cargo tree 验证）。
决策 2 的存在理由至此兑现：忘了重新生成，cargo test 就红。