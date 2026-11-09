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

### 3. UI follows DESIGN.md

Any change to the interface follows [DESIGN.md](DESIGN.md): the theme tokens
and kit components, copy (let the layout do the explaining), spacing, colour,
icons, type, layout, details and motion.

### 4. Code follows CODE-DESIGN.md

Before designing, writing, refactoring or reviewing code, read
[CODE-DESIGN.md](CODE-DESIGN.md): module depth and readability, state and
async work, files, and the checks to run before you finish.

### 5. Commits follow Conventional Commits

Release notes are built from commit messages and pull request titles on
`main` ([release-notes.sh](.github/scripts/release-notes.sh)), so write them
for the people who use Flikk.

```text
type(scope): summary

What changed for the user and why, in one short paragraph.

Anything else a reviewer needs: details, trade-offs, follow-ups.
```

- **type** picks the release notes section:

  | Type | Use for | Section |
  | --- | --- | --- |
  | `feat` | Something new the user can do or see | New features |
  | `fix` | A bug the user could hit | Fixes |
  | `perf` | Faster or lighter, same behaviour | Performance |
  | `refactor` | Code change with no visible effect | Internal |
  | `docs` | Documentation only | Internal |
  | `build`, `ci` | Cargo, packaging, workflows | Internal |
  | `chore` | Anything else that users won't notice | Internal |

- **scope** is optional: the area touched, in lower case, such as
  `timeline`, `agent`, `export`, `settings`, `linux`.
- **summary** is imperative, at most 72 characters, no trailing period:
  `feat(timeline): snap clips to the playhead`.
- **Breaking changes** add `!` after the type or scope (`feat(export)!: …`)
  and explain the change in a `BREAKING CHANGE:` footer.
- **The first body paragraph** appears in the release notes under the
  summary. Say what the user gets, not which functions moved.
- **Pull request titles** follow the same format: a merged pull request is
  listed under its title and the first paragraph of its description.

## Commands

| Task | Command |
| --- | --- |
| Build | `cargo build --workspace` |
| Run | `cargo run -p flikk` |
| Format check | `cargo fmt --check` |
| Format | `cargo fmt` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Tests | `cargo test --workspace` (expected to run 0 tests) |
