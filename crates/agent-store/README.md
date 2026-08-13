# einfach-store

An atomic dependency graph with a command log, in Rust.

State lives in atoms. Derived atoms are pure functions of other atoms and recompute
glitch-free when their inputs change. Every write goes through a command layer that
records the old and new value — so **undo, redo, snapshot recovery and audit replay are
the same mechanism, not four features you have to keep in sync.**

```toml
[dependencies]
einfach-store = "0.1"
```

## A counter and a doubled view of it

Values are your type; you implement `AtomValue` for it. Nothing here is specific to any
particular application — the store only knows about atoms, dependencies, and the log.

```rust
use einfach_store::Store;

// Your value type. `null()` is the placeholder used for an atom whose recompute
// exceeded the recursion budget.
#[derive(Clone, PartialEq, Debug)]
enum Val {
    Num(i64),
    Null,
}
impl einfach_store::AtomValue for Val {
    fn null() -> Self {
        Val::Null
    }
}

let store: Store<Val> = Store::new();

let count = store.create_atom(Val::Num(1));
let doubled = store.create_derived(move |get| match get(count) {
    Val::Num(n) => Val::Num(n * 2),
    Val::Null => Val::Null,
});

assert_eq!(store.get(doubled), Val::Num(2));

store.set(count, Val::Num(21));
assert_eq!(store.get(doubled), Val::Num(42)); // recomputed, once
```

`create_derived` takes a read function that must be **pure**. That is not a style
preference: undo replays the log and recomputes derived values, so a clock or a random
number inside a read function produces a different value on replay than it did the first
time — and nothing errors. The bug shows up as a silently wrong value after an undo, weeks
later.

## What it is for

The interesting part is not the reactivity — it is that the log is the only way state
changes, so anything you can express as "replay the log to position N" you get for free:

- **undo / redo** — move a cursor along the log
- **crash recovery** — load the last snapshot, replay forward; that loop *is* redo
- **audit replay** — the log is already the audit trail, not a second thing to maintain

## Status

`0.1.0`. The engine is exercised by a large test suite and used in production inside
[einfach-agent](https://github.com/allroad88888888/einfach-agent-rust), but **the public
API is not stable yet** and will change before 1.0.

## Lineage

Forked from the Rust atomic engine in
[einfach](https://github.com/allroad88888888/einfach) (`excel/rust/core`), then evolved
independently — it does not track upstream and does not pick up upstream bug fixes
automatically.

Removed at fork time: the spreadsheet heritage (rectangular `ArrayData` blocks, lambda
values, Excel error codes). Kept deliberately: synchronous re-entrant semantics, the
glitch-free pending-queue propagation, the 256-deep recursion budget, and `AtomFamily`.

## License

[Apache-2.0](../../LICENSE-APACHE) or [MIT](../../LICENSE-MIT), at your option.
