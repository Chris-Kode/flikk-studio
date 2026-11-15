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

### 6. Documentation follows ASD-STE100

Write all documentation in ASD-STE100 Simplified Technical English: the
files in [docs/tech-spec](docs/tech-spec/README.md), README files and other
Markdown documents. This rule does not apply to interface copy
([DESIGN.md](DESIGN.md)) or to commit messages (rule 5).

- **Words:** Use a word in one meaning only, and use the same word for the
  same thing each time. Use simple verbs: "use", not "utilize" or "make
  use of". Do not use slang or idioms.
- **Technical names:** Names of code, crates, files, tools and products
  are permitted. Write them as they are in the code.
- **Sentences:** An instruction has a maximum of 20 words. A description
  has a maximum of 25 words. Write one topic in each sentence.
- **Paragraphs:** A paragraph has one topic and a maximum of six
  sentences.
- **Verbs:** Use the active voice. Use the simple present, simple past
  and simple future tenses. Do not use the "-ing" form of a verb as a
  verb.
- **Instructions:** Write an instruction as a command: "Add the row."
  Write one instruction in each sentence. Put the condition first: "If
  the GPU fails, the CPU draws the frame."
- **Noun clusters:** Use a maximum of three nouns together.
- **Lists and tables:** Use a list for steps or items that are parallel.
  Use a table to compare items.
- **Contractions:** Do not use contractions.

Each new file in `docs/tech-spec` must also obey the rules in its
[README](docs/tech-spec/README.md).

## Commands

| Task | Command |
| --- | --- |
| Build | `cargo build --workspace` |
| Run | `cargo run -p flikk` |
| Format check | `cargo fmt --check` |
| Format | `cargo fmt` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Tests | `cargo test --workspace` (expected to run 0 tests) |
