# 267 test files, 58 GB of `target/`, and the fix that didn't hold

*A Rust build-bloat story with two halves. The first half is the one everyone tells. The second half is the one worth reading.*

---

## Part one: the story you've heard

Two days into a Rust project, builds got slow. Not "I should get coffee" slow — minutes of apparent idling before `rustc` printed anything.

`du -sh target/` said **58 GB**. `find target -type f | wc -l` said **880,000 files**.

The project had 128k lines of Rust and existed for 48 hours.

### The cause

Every `.rs` file directly under `tests/` is its own crate. Cargo compiles it into its own binary, and each of those binaries links the entire dependency tree.

There were 267 of them.

267 separate link steps, 267 separate copies of shared dependency code, 267 sets of debug info — for a test suite that could have been one binary.

The second factor was file *count*, not size. `rustc` enumerates `target/debug/deps` on startup. At a few thousand entries that's free. At several hundred thousand it is not: **the build had become slow because of how much the previous builds had left behind.**

### The fix

Two changes.

**One test harness per crate.** Instead of `tests/foo.rs`, `tests/bar.rs`, …, put everything under `tests/it/` with a single `tests/it/main.rs`:

```rust
//! Single integration-test harness for this crate: every case compiles into one binary.
//! Why merged: 267 single-file tests = 267 link products + 267 process launches.
//! Adding a test = create a file under tests/it/ and add one `mod` line here.

mod cancel;
mod undo_redo_roundtrip;
mod snapshot_recovery;
// ...
```

Adding a test is now "create the file, add one line." 296 files moved in a single commit.

**Stop emitting full debug info.** Panic backtraces need line numbers, not full type information. Third-party crates don't even need line numbers — you're not going to single-step through them:

```toml
[profile.dev]
debug = "line-tables-only"

[profile.dev.package."*"]
debug = false
```

Builds got fast again — touching one file in the deepest crate and rebuilding the whole workspace takes **6 seconds** today. The incident got written into the project's contributor docs so nobody would recreate it. Case closed.

---

## Part two: eight days later

I went to measure `target/` again — I wanted a clean before/after number for this very article.

```
target/debug/incremental   16 GB     ← largest
target/debug/deps          14 GB
target/debug/examples     688 MB
─────────────────────────────────
target/                    31 GB / 794,507 files
```

Thirty-one gigabytes. Nearly eight hundred thousand files. **The number I was going to use as the "after" was almost the "before."**

But look at *where* it is. `deps` — the thing the fix targeted — is 14 GB and no longer the biggest item. The fix worked. Something else grew into the space.

`incremental/` held **708 session directories**, the largest a single 151 MB.

Incremental compilation state is keyed by crate *and* build configuration. This project has more configurations than most: a `ts` feature flag, `--all-targets`, and three compilation targets. Every combination gets its own incremental state, and each keeps history.

I deleted it. 16 GB freed, `target/` down to 15 GB, and I started writing this section.

### Then someone asked why it was still 15 GB

Fair question. `deps/` was 14 GB of that. Broken down:

| | |
|---|---|
| `.rlib` — the actual build products | 1.7 GB |
| `.rmeta` | 906 MB |
| **`.rcgu.o` — intermediate object files** | **~11.4 GB, in 631,526 files** |

`.rcgu.o` is one object file per codegen unit. The dev profile defaults to 256 codegen units, and every distinct build hash gets its own set — **and Cargo never reclaims the sets belonging to hashes that are no longer current.**

One crate, `agent_cli`, had accumulated **40 build hashes and 42,732 object files.** One of those 40 was live. The other 39 were sediment from builds I'd run over the previous two weeks.

Object files are inputs to a link step that already happened; the `.rlib` next to them is the result. Deleting them costs, at worst, regenerating them on the next build.

So: incremental wasn't the biggest thing. It was the **most legible** thing — one directory with an obvious name and an obvious purpose. The larger problem was 631,526 files with machine-generated names that no tool was ever going to bring to my attention.

Final state, across all four workspaces in this repo: **35 GB → 9 GB**, and `deps/` went from **631,526 files to 3,227** — roughly 200×.

That second number is the one that matters. `rustc` enumerates `deps/` on startup. File *count*, not size, is what made builds slow in the first place.

Nobody did anything wrong at any point. There was no second mistake. The disk filled up through ordinary work, twice, in two different places.

---

## What I actually take from this

**The lesson isn't "watch your test file layout."** That was the surface cause, and fixing it was correct — `deps` really did stop being the problem *for that reason*.

The lesson is that **`target/` is a cache with no eviction policy and no budget**, and a fix aimed at one contributor to its growth doesn't change that. I fixed a symptom precisely and thoroughly, wrote it up carefully, and left the underlying property untouched: nothing in the system has an opinion about how large `target/` should be.

There's a sharper version of this, and it's the part I'd want someone to take away.

**I found the 16 GB because it had a name.** `incremental/` is one directory, it means something, and it showed up at the top of `du -sh target/debug/*`. The 11.4 GB sitting next to it was 631,526 files called things like `agent_cli-06e2d0425d1d64a9.2te8wc0c7sgg9x7bhuy3qs9pn.0bq0m75.rcgu.o`, and it only surfaced because a second person looked at the number I'd published and asked why it was still that big.

Legibility and size are not correlated. The thing that's easy to notice is not the thing that's biggest — and if your only detection mechanism is "eventually somebody looks," you will find the legible problem and stop.

So the fix I actually needed was never "reorganize the tests," and it wasn't "delete `incremental/`" either. It was **a number that gets checked on a schedule** — so that "how big is `target/`?" is a question something asks by itself, rather than one I stumble into while writing a blog post eight days later, and then only half-answer.

That's the difference between fixing an incident and fixing the thing that produced it. I've now done the first one twice and I'm still not done with the second.

---

## If you want the cheap wins anyway

They're real, they're two minutes of work, and they helped:

1. **One test harness per crate.** `tests/it/main.rs` with `mod` lines. Do this before you have 267 files, not after.
2. **`debug = "line-tables-only"`**, and `debug = false` for `[profile.dev.package."*"]`. Panic traces stay readable.
3. **Actually measure**: `du -sh target/` and `find target -type f | wc -l`. The file count is the one that surprises people — and it's the one that makes `rustc` slow before the disk fills up.

And then set something up that checks the number for you. That's the part I'm still missing.
