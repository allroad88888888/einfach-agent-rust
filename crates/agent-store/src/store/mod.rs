//! Faithful Rust port of `@einfach/core` 的 `store.ts`（上游主仓的手写 atom store，
//! 源码不在本仓 —— 见 `docs/decisions/0002-upstream-core-via-npm.md`）。 Function-per-function isomorphism is INV-1 of
//! `excel/rust/docs/ATOM_DELEGATION_REWRITE_PLAN.md`:
//!
//! | store.ts            | store.rs                    |
//! |---------------------|-----------------------------|
//! | `readAtom`          | `Inner::read_atom` (iterative frame loop) |
//! | `setAtom`           | `Store::set` → `set_atom`   |
//! | `writeAtomState`    | `Inner::write_atom_state`   |
//! | `setAtomState`      | `Inner::set_atom_state`     |
//! | `dependenciesChange`| `Inner::dependencies_change` (iterative) |
//! | `flushPending`      | `flush_pending`             |
//! | `publishAtom`       | `publish_atom`              |
//! | `subscribeAtom`     | `Store::sub`                |
//! | `clearDependencies` | commit-time dep diff in `commit_read` |
//! | `clear`             | `Store::clear` (incl. audit C-7 pending purge) |
//!
//! Permitted mechanical deviations (each also a ledger row in the WORKPLAN):
//! - DV-1: no Promise machinery — async branches of setAtom/setAtomState/
//!   dependenciesChange do not exist here (`Value` has no async variant).
//! - DV-2: `Object.is` reference snapshots become per-atom GENERATION
//!   counters. `generation` increments exactly when store.ts would replace
//!   the stored reference (a value-changing `setAtomState`), so
//!   `gen == snapshot` ⟹ `Object.is` would pass. The converse differs only
//!   under ABA, costing one spurious re-derive that equality-pruning absorbs.
//!   Additionally Rust `PartialEq` (with `Arc::ptr_eq`/NaN fast paths in
//!   `Value`) prunes strictly MORE than reference equality — fewer publishes
//!   for structurally-equal replacements, never fewer recomputations of
//!   changed values.
//! - DV-3: the recursive `readAtom` getter-pull and `dependenciesChange`
//!   walk are implemented with explicit work stacks plus a NeedsDep
//!   scratch-commit protocol (see `read_atom`), because 100k-deep formula
//!   chains overflow a 1 MB WASM stack. Semantics are unchanged: a faulted
//!   read discards its scratch (committed deps stay intact — preserving the
//!   store.ts:47-51 "cached value with no dep entry is unconditionally
//!   fresh" behavior), computes the missing deps bottom-up, then re-runs.
//!   Recompute counters bump ONLY on completed runs.
//! - DV-4: `settled-memo` — a global `write_seq` plus per-atom `settled_at`
//!   lets `dependencies_change` skip re-validating an atom that was already
//!   confirmed fresh at the current write sequence. Pure memoization of a
//!   deterministic check (no value can have moved in between); required to
//!   keep bulk writes into shared dependents O(N + E) instead of O(N·deps).
//! - Store-level cross-atom cycle detection panics (store.ts would
//!   stack-overflow; the excel engine detects cycles at the evaluator level
//!   and never lets the store see them). Self-reads return the cached/init
//!   value without an edge, exactly like store.ts:97-102.
//! - `batch()` is the explicit form of what a vanilla write-function body
//!   gets implicitly (sets deferred, one flush at the end); kept because
//!   the engine already uses it.
//!
//! ## 008 拆分后的模块地图
//!
//! 上游单文件按职责拆成子模块（`docs/issues/008-split-store.md`）。每个子模块只对
//! `store` 内部 `pub(super)`，crate 的公开面仍然只有下面这几个 `pub use` —— 和拆分前
//! 完全一致，调用方感知不到这次重排。
//!
//! | 子模块 | 职责 |
//! |--------|------|
//! | `records` | atom 记录与依赖图的数据结构（`AtomRecord`/`BackDeps`/`Inner`）及其最基础的存取 |
//! | `handle`  | `Store` 句柄本身：构造、以及 primitive/derived/writable atom 的创建 |
//! | `graph`   | 面向 engine adapter 的图结构查询与 atom 销毁（`invalidate`/`reverse_*`/`destroy_atom`/`clear`） |
//! | `guards`  | 四个 RAII 守卫：panic 不清空 `computing`/`read_depth`/`setting`/`batch_depth` 就会永久卡死 |
//! | `read`    | 喂给 derived 读函数的追踪/免追踪访问口：`ReadArgs`、`Scratch` 暂存区、`read_dep` |
//! | `eval`    | DV-3 迭代式求值状态机本体：`read_atom` 的显式帧栈、`commit_read`、`seed_primitive` |
//! | `flush`   | 写入落地后的 pending 调度与依赖传播：`WriteArgs`、`flush_pending`、`dependencies_change` |
//! | `subscribe` | atom 变更的订阅登记与分发 |
//! | `debug`   | 面向诊断的只读探针（`#[doc(hidden)]`） |

mod debug;
mod eval;
mod flush;
mod graph;
mod guards;
mod handle;
mod read;
mod records;
mod subscribe;

pub use flush::WriteArgs;
pub use handle::Store;
pub use read::ReadArgs;
pub use records::AtomValue;
pub use subscribe::{CellListener, SubscriptionId};
