# Persistence

- **Project files** (`flikk-store/src/project_file.rs`) are `.flikk` JSON
  (`format: "flikk-project"`, `version: 1`). The code writes them
  atomically. Each media item keeps its absolute path and its path
  relative to the file. The code tries the paths in that sequence.
  - Agent settings belong to the user and apply to all projects. The file
    does not keep them. Older files have a `settings` key, which the code
    ignores.
  - The project menu (`flikk/src/projects.rs`) has New, Open, Save, Save As
    and recent files (cmd/ctrl-N, O, S, shift-S).
  - The menu shows a dot while there are unsaved changes.
  - Before New, Open, Quit or a window close, the menu asks a Save / Don't
    Save / Cancel question.
- **Autosave and backups:** Unsaved changes autosave to
  `paths::autosave()/<id>.flikk` for crash recovery. The autosave occurs
  1 s after the last change, and a maximum of 5 s after the first. Saved
  files get a backup in `paths::backups()` a maximum of one time in 5
  minutes. The code keeps 10 backups.
- **Offline media** gets a notice. The user relinks it from a folder
  (`flikk/src/relink.rs`). The code matches the file name. When possible,
  it also confirms the match by fingerprint.
- **Database** (`paths::database()`, SQLite in WAL mode,
  `flikk-store/src/schema.rs`) has these tables:
  - `meta`: agent settings, model selections, analysis steps, the provider
    for each capability and recent projects.
  - `style_profiles` (caption styles), `style_rules`, `source_facts` and
    `media_fingerprints`.

  The database has no data for each project. Imported Markdown notes
  become proposed style rules, one rule for each bullet or paragraph (S5).
  When the media of a path is replaced, the code suspends the facts for
  that path (F2).
- **Caption styles** (`CaptionStyle`, `flikk-core/src/memory.rs`) have
  these properties:
  - Font, weight, spacing, fill, stroke, shadow, background box, position
    and nudge.
  - A highlight for the spoken word (color, box, karaoke, reveal word by
    word).
  - Intro/outro motion (fade, pop, slide).

  There are twelve ready-made looks. Sizes are at 1080p. The code scales
  them by the short side of the frame.
- **Other user files** under `paths::home()`:
  - `projects/` (chats)
  - `library/`, with `library/assets/` (the audio and voice samples of the
    library)
  - `models/` and `cache/`
  - `agent/` (sessions, models, tool groups, tool usage, work dirs)
  - `generated/` (music, sound effects and voiceovers, with their `.json`)
  - `logs/`
  - `updates/` (a new version while it downloads and unpacks)
  - `bridge.json`
  - `credentials.json` (owner-only, only where the system has no
    credential store)

## Releases and updates

- **Release** (`.github/workflows/release.yml`) runs by hand on `main`. It
  does these steps:
  1. It increases the workspace version (patch, minor or major) in
     `Cargo.toml` and in the entry of each member in `Cargo.lock`.
  2. It commits "Release vX.Y.Z" to `main` and tags it.
  3. It publishes a GitHub release with generated notes.
  4. It starts Deploy. A release that `GITHUB_TOKEN` makes does not start a
     `release` workflow.
- **Deploy** (`.github/workflows/deploy.yml`) runs on a published release,
  or by hand with a tag.
  - It builds `flikk` and `flikk-mcp` in release mode on three platforms:
    Linux x86-64 (Ubuntu 24.04), macOS Apple silicon (14.0 and later) and
    Windows x86-64. `FLIKK_UPDATES` and `FLIKK_UPDATE_REPO` are set.
  - It attaches the output of `scripts/package.sh`:
    `flikk-<v>-<os>-<arch>.tar.gz` (`.zip` on Windows) and its `.sha256`.
  - On macOS, the archive holds `Flikk.app`. The app has an ad hoc
    signature and is not notarized. `mlx.metallib` is adjacent to the
    binary.
  - On other platforms, the archive holds a folder. The folder contains the
    binaries and the libraries that cargo put adjacent to them (Dawn on
    Linux).
  - Windows also gets `…-setup.exe`, which Inno Setup builds from
    `scripts/windows/flikk.iss`. The setup installs for the user alone into
    `%LOCALAPPDATA%\Programs\Flikk`, without an administrator. Thus updates
    can write there. The setup adds a Start menu entry and opens `.flikk`
    files.
  - Deploy fails when the tag is not the crate version.
  - On Windows, Deploy builds into `D:\t`. The reason is the Vulkan shader
    generator of whisper.cpp. It is a CMake project deep in the build
    folder of cargo. Under the checkout, its paths have more than 260
    characters. The file tracker of MSBuild cannot open such paths (error
    FTK1011).
  - A local Windows build in a deep folder also needs a short
    `CARGO_TARGET_DIR`.
  - Deploy builds the source of the tag. But it packages with `scripts/`
    from its own commit (checked out into `.deploy/`). Thus, if you
    rebuild a release by hand from `main`, it gets the packaging fixes made
    after the tag.
- **Updates** (`flikk/src/update.rs`, `flikk/src/state/update.rs`):
  - Only packaged builds (`FLIKK_UPDATES` at build time) ask GitHub for the
    latest release. They ask at launch and every 6 hours.
    `FLIKK_NO_UPDATES` stops these requests.
  - If a newer version has the archive for this platform, a notice shows
    under the bell. The user cannot dismiss it, and it is first in the
    list.
  - Update downloads the archive to `paths::updates()` and checks its
    SHA-256. It unpacks the archive with the `tar` of the system, which
    reads zip on Windows.
  - Update then puts the files over the install. On macOS, this is the
    bundle. On other platforms, it is the folder of the executable.
  - The code writes each file adjacent to its target, and then renames it
    over the target.
  - On Windows, the code first renames the target to a different name
    (`*.flikk-old`). The next launch removes these files. The reason is
    that Windows can rename a running executable, but cannot replace it.
  - Restart handles unsaved changes as Quit does. Then it starts the
    installed version.
  - When the code finds the release, it tries to write to the install
    folder. Some folders are not writable, for example Program Files,
    `/opt` and `/Applications` for a standard user. If the folder is not
    writable, the notice shows Download… in place of Update. This also
    occurs if an update failed and the folder then is not writable.
    Download… opens the page of the release.
