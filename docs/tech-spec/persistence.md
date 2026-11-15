# Persistence

* **Project files** (`flikk-store/src/project_file.rs`): `.flikk` JSON
  (`format: "flikk-project"`, `version: 1`), written atomically; media keeps
  its absolute path and its path relative to the file, tried in that order.
  Agent settings are the user's, for every project, and are not saved in
  the file (a `settings` key older files carry is ignored). New, Open, Save,
  Save As and recent files (cmd/ctrl-N, O, S, shift-S) are in the project
  menu (`flikk/src/projects.rs`), with a dot while there are unsaved
  changes and a Save / Don't Save / Cancel question before New, Open, Quit
  or closing the window.
* **Autosave and backups:** unsaved changes autosave to
  `paths::autosave()/<id>.flikk` (1 s after the last change, at most 5 s after
  the first) for crash recovery; saved files get a backup in
  `paths::backups()` at most every 5 minutes, 10 kept.
* **Offline media** is flagged with a notice and relinked from a folder
  (`flikk/src/relink.rs`): matched by file name, confirmed by
  fingerprint when it can be.
* **Database** (`paths::database()`, SQLite in WAL mode,
  `flikk-store/src/schema.rs`): `meta` (agent settings, model picks, analysis
  steps, the provider per capability, recent projects), `style_profiles` (caption styles), `style_rules`,
  `source_facts` and `media_fingerprints`. Nothing per project.
  Imported Markdown notes become proposed style rules, one per bullet or
  paragraph (S5); facts tied to a path are suspended when its media is
  replaced (F2).
* **Caption styles** (`CaptionStyle`, `flikk-core/src/memory.rs`): font,
  weight, spacing, fill, stroke, shadow, background box, position and
  nudge, a spoken-word highlight (color, box, karaoke, word-by-word reveal)
  and intro/outro motion (fade, pop, slide), with twelve ready-made looks;
  sizes are at 1080p, scaled by the frame's short side.
* **Other user files** under `paths::home()`: `projects/` (chats),
  `library/` (with `library/assets/`, the library's audio and voice
  samples), `models/`, `cache/`, `agent/` (sessions, models, tool groups,
  tool usage, work dirs), `generated/` (music, sound effects and
  voiceovers with their `.json`), `logs/`, `updates/` (a new version
  while it downloads and unpacks), `bridge.json`, and `credentials.json`
  (owner-only, only where the system has no credential store).

## Releases and updates

* **Release** (`.github/workflows/release.yml`, run by hand on `main`):
  raises the workspace version (patch, minor or major) in `Cargo.toml` and
  every member's entry in `Cargo.lock`, commits "Release vX.Y.Z" to `main`,
  tags it and publishes a GitHub release with generated notes, then starts
  Deploy (a release made with `GITHUB_TOKEN` starts no `release` workflow).
* **Deploy** (`.github/workflows/deploy.yml`, on a published release or by
  hand with a tag): builds `flikk` and `flikk-mcp` in release on Linux
  x86-64 (Ubuntu 24.04), macOS Apple silicon (14.0 and later) and Windows
  x86-64, with `FLIKK_UPDATES` and `FLIKK_UPDATE_REPO` set, and attaches
  what `scripts/package.sh` makes: `flikk-<v>-<os>-<arch>.tar.gz` (`.zip`
  on Windows) and its `.sha256`. On macOS the archive holds `Flikk.app`
  (ad hoc signed, not notarized, `mlx.metallib` beside the binary);
  elsewhere a folder with the binaries and the libraries cargo put beside
  them (Dawn on Linux). Windows also gets `…-setup.exe`, built with Inno
  Setup from `scripts/windows/flikk.iss`: it installs for the user alone
  into `%LOCALAPPDATA%\Programs\Flikk`, without an administrator, so
  updates can write there, adds a Start menu entry and opens `.flikk`
  files. It fails when the tag is not the crate version. On Windows it
  builds into `D:\t`: whisper.cpp's Vulkan shader generator is a CMake
  project nested so deep in cargo's build folder that under the checkout
  its paths pass 260 characters, which MSBuild's file tracker can't open
  (error FTK1011). A local Windows build in a deep folder needs a short
  `CARGO_TARGET_DIR` too. Deploy builds the tag's source but packages
  with `scripts/` from its own commit (checked out into `.deploy/`), so a
  release rebuilt by hand from `main` gets packaging fixes made since.
* **Updates** (`flikk/src/update.rs`, `flikk/src/state/update.rs`): only
  packaged builds (`FLIKK_UPDATES` at build time) ask GitHub for the
  latest release, at launch and every 6 hours; `FLIKK_NO_UPDATES` turns
  that off. A newer version with this platform's archive is a notice under
  the bell that can't be dismissed and is listed first. Update downloads
  it to `paths::updates()`, checks its SHA-256, unpacks it with the
  system's `tar` (which reads zip on Windows) and lays its files over the
  install: the bundle on macOS, the executable's folder elsewhere. Each
  file is written beside its target and renamed over it; on Windows the
  target is renamed aside first (`*.flikk-old`, swept at the next launch),
  since a running executable can be renamed but not replaced. Restart
  settles unsaved changes like Quit, then starts the installed version.
  Whether the install folder can be written to is tried when the release
  is found: where it can't (Program Files, `/opt`, `/Applications` for a
  standard user), or where an update failed and it then can't, the notice
  offers Download…, which opens the release's page, in place of Update.
