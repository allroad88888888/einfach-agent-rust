// 032 验收原文：「TS 侧一个 fixtures.test.ts（tsc 层面）」——这不是一个跑起来的
// 测试文件，`pnpm -r typecheck` 就是唯一的断言器：`fixtures/` 里任意一条样本的
// 形状跟 `Frame`（034 起的 agent 归属信封——`{ agent, event }`，`event` 是
// `SessionEvent` 判别联合）对不上，`tsc --noEmit` 直接报错在下面这一行，不需要
// 任何测试框架。
//
// 数据从 `../fixtures/events.ts` 导入，**不是**验收原文写的
// `../fixtures/events.json`——这是刻意的偏差，原因不是偷懒，是 TypeScript 对
// JSON 模块 import 的既有行为逼的：JSON import 的字符串字面量类型一律加宽成
// `string`（`"gap"` 变成 `string`），邻接标签的判别字段和几乎每个嵌套枚举都是
// 字符串字面量联合，加宽之后 `satisfies` 不管协议形状对不对都会红，检查直接
// 失去意义（拿本仓当前工具链实测过，见 crates/agent-server/src/ts_protocol/
// fixtures.rs 模块文档，那里记了实测过程）。`events.ts` 内嵌的是跟
// `events.json` 字节相同的内容（同一次 Rust 生成，见那份文档），只是外面套了
// `as const`——这是 TS 里唯一能让字面量类型不被加宽的写法，且只对写在源码里的
// 字面量表达式生效，对 import 进来的值不起作用，所以不能对着 `.json` 补救。
import { events } from "../fixtures/events";
import type { Frame } from "./generated/Frame";

/// `as const` 会把数组变成 `readonly` 元组、把每个属性标成 `readonly`——这跟
/// `Frame[]`（普通可变数组）在「能不能互相赋值」这件事上专门被 TS 拦一道
/// （`error TS4104`），跟形状对不对没关系，纯粹是可变性标记不匹配。`Mutable<T>`
/// 递归剥掉 `readonly`，只改可变性、不改字面量类型——`events` 的字面量精度全须
/// 全尾地留到下面 `satisfies` 那一行，检查还是真检查。
type Mutable<T> = T extends readonly (infer E)[]
  ? Mutable<E>[]
  : T extends object
    ? { -readonly [K in keyof T]: Mutable<T[K]> }
    : T;

(events as unknown as Mutable<typeof events>) satisfies Frame[];
