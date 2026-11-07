# AGENTS.md

Instructions for AI coding agents in this repository. Claude Code reads
[CLAUDE.md](CLAUDE.md), which points here; treat this file as the source of
truth.

## What this is

Flikk is a native desktop editor: Rust + GPUI-KIT (Capcut-style).


## Hard rules

### 1. Do not write tests

This project deliberately has no tests. Never add:

- `#[test]` functions, `#[cfg(test)]` modules, or `tests/` directories;
- benches, or test-only fixtures and helpers.

Many issues list unit or integration tests in their acceptance criteria. Skip
those items and say so in your summary. `cargo test --workspace` must pass by
having nothing to run.


### 2. User data lives in `~/.flikk/`

Never write inside a reviewed repository. Honor `FLIKK_HOME` for anything that
resolves a user-data path, and use it in manual checks. The only files written
elsewhere are the ones the user saves there: `.flikk` project files and exports.
Autosaves, backups and the shared library stay under the user-data folder.

```sh
FLIKK_HOME=/tmp/flikk-dev cargo run -p flikk
```

## State and async work

### Make the states explicit

- Model a lifecycle as an `enum` whose variants carry their data
  (`Idle`, `Loading { read }`, `Loaded { .. }`, `Failed { at, message }`), not
  as `Option`, `bool` and counter fields that can contradict each other.
- `None` never means two things: waiting, in flight, done and failed are
  separate states.
- A cancelled job is not a failed job: don't record it as a failure or show
  it as failed.

### Async results must still be wanted

- Anything that crosses an `await`, a thread or a channel carries what it was
  for: a request counter, turn id, project id or bus version. On arrival,
  drop it if that no longer matches. Bump the counter on every new request,
  set or removal.
- Check the project after every await. A result for a project that is no
  longer open is kept for that project or dropped, never applied to the
  current one.
- A background job that proposes whole values records `ToolCtx::version()`
  when it takes its snapshot and proposes through `propose_job` /
  `execute_since`, so edits made since conflict instead of being overwritten.

### Every in-progress state has a guaranteed exit

- Workers and jobs use `catch_unwind` per item, plus a drop guard that resets
  "running" flags and wakes waiters.
- Child processes are wrapped in a guard that kills and waits on drop. A
  reader error kills the process; the rest of its output is not drained.
- A failure is retried after a back-off (about 30 s), never cached for the
  session.
- Long work takes a cancel (`&AtomicBool`, `JobProgress::cancelled()`,
  `GenerationTask`) and checks it in its loops, its waits and its retry
  sleeps.
- When media leaves the project or the project switches, its background work
  is cancelled.

### UI interactions end in one place

- An edit commits to the target it began on (see `Target` in
  `ui/inspector.rs`): GPUI delivers blur after render, when the selection may
  already have changed.
- Every drag has a single end path. A move with the button up counts as a
  release. Snap lines and hover state are cleared, and the drag is cancelled
  if its target disappears or playback starts.
- A live preview (`show_live`, `caption_live`) is cleared on every drag end,
  whether or not it commits, on a target change and on a project switch.
- UI state is keyed by ids, not list indexes (messages, words, chips, rows).
- Per-project state is reset in one function that every switch path calls
  (`switch_to`, `reset_chat_view`).

### One rule, one place

- Shared rules live in one function and are called, never re-derived: "is
  this track heard" (`Timeline::audible`), "does this clip fit this track"
  (`edit::fits_track`), "is the project dirty" (the undo position).
- Project changes go through the `CommandBus` with the right `Author`. User
  edits are `Author::User`. Agent results the user already approved use
  `execute_shared_as` with the agent as author.

### Files

- Write through a temp file and a rename. Claim a free name atomically
  (`create_new`, a hard link or a placeholder), never with `exists()`
  followed by a write.
- Never overwrite a file this operation didn't create. Remove temp and part
  files on error, and sweep the ones a crash leaves.
- Don't swallow errors with `.ok()?` on export or save paths: a render fails
  with a message rather than silently leaving something out.

### Before you finish

For every state you added or changed:

1. Can two of its fields contradict each other? Make it an enum.
2. What happens if the result arrives late, twice, or for another project?
3. What happens on panic, cancel and failure, and when the user switches
   project or chat?
4. Can a path leave a flag, preview, file or process behind?
5. Is this rule already written somewhere else?

## Commands

| Task | Command |
| --- | --- |
| Build | `cargo build --workspace` |
| Run | `cargo run -p flikk` |
| Format check | `cargo fmt --check` |
| Format | `cargo fmt` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Tests | `cargo test --workspace` (expected to run 0 tests) |
