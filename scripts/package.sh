#!/usr/bin/env bash
# Package a release build of Flikk for this platform into dist/, with a
# `sha256sum` line beside it:
#
#   cargo build --release --locked -p flikk -p flikk-agent
#   scripts/package.sh 0.2.0
#
#   macOS    dist/flikk-0.2.0-macos-aarch64.tar.gz    Flikk.app
#            dist/flikk-0.2.0-macos-aarch64.dmg, a disk image to drag
#            Flikk.app to Applications from
#   Linux    dist/flikk-0.2.0-linux-x86_64.tar.gz     flikk-0.2.0-linux-x86_64/
#   Windows  dist/flikk-0.2.0-windows-x86_64.zip      flikk-0.2.0-windows-x86_64/
#            dist/flikk-0.2.0-windows-x86_64-setup.exe, a per-user installer
#            (scripts/windows/flikk.iss) built with Inno Setup 6, when
#            ISCC is installed; required on CI
#
# Every package includes FFmpeg, downloaded into dist/cache on the first
# run.
#
# The updater (crates/flikk/src/update.rs, `asset_name`) looks for these
# names, and lays the archive's one folder over the installed copy.
set -euo pipefail
# Say which command failed. macOS runs this with bash 3.2, which can stop
# a script under `set -e` without running this trap, so on CI every
# command is traced as well.
trap 'echo "package.sh:$LINENO: \`$BASH_COMMAND\` failed" >&2' ERR
if [ -n "${CI:-}" ]; then
  echo "package.sh: bash $BASH_VERSION"
  set -x
fi

version=${1:?usage: scripts/package.sh <version> [binary dir]}
bin=${2:-target/release}
# This script's folder: Deploy runs a copy from another commit than the
# source it packages.
here=$(cd "$(dirname "$0")" && pwd)

case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  MINGW* | MSYS* | CYGWIN*) os=windows ;;
  *) echo "unsupported system: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

name="flikk-$version-$os-$arch"
dist=dist
stage="$dist/stage"
rm -rf "$stage"
mkdir -p "$stage"

sha256() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# Download $1 into dist/cache once, checked against SHA-256 $2; the path
# is left in $fetched.
fetch() {
  fetched="$dist/cache/$2-$(basename "$1")"
  if [ -f "$fetched" ]; then
    return 0
  fi
  mkdir -p "$dist/cache"
  curl -fsSL --retry 3 -o "$fetched.part" "$1"
  got=$(sha256 "$fetched.part")
  if [ "$got" != "$2" ]; then
    echo "$1: SHA-256 $got, expected $2" >&2
    rm -f "$fetched.part"
    exit 1
  fi
  mv "$fetched.part" "$fetched"
}

# FFmpeg ships beside the editor, which uses it over one on PATH
# (crates/flikk/src/bundled_ffmpeg.rs): static GPL builds of the same
# release, with hardware encoders (VideoToolbox; NVENC, Quick Sync, AMF,
# VA-API). $1 is the folder, $2 the builder's page, $3 the source if not
# the release's tarball.
ffmpeg_version=9.0.2
ffmpeg_notice() {
  fetch https://www.gnu.org/licenses/gpl-3.0.txt \
    3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986
  cp "$fetched" "$1/FFmpeg-GPL-3.0.txt"
  cat >"$1/FFmpeg.txt" <<EOF
Flikk runs FFmpeg $ffmpeg_version (https://ffmpeg.org), shipped with it
unmodified as the ffmpeg and ffprobe programs. They are licensed under the
GNU General Public License, version 3: FFmpeg-GPL-3.0.txt.

Source: ${3:-https://ffmpeg.org/releases/ffmpeg-$ffmpeg_version.tar.xz}
Built by: $2
EOF
}

case $os in
  macos)
    app="$stage/Flikk.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$bin/flikk" "$bin/flikk-mcp" "$app/Contents/MacOS/"
    find "$bin" -maxdepth 1 -name '*.dylib' -exec cp -L {} "$app/Contents/MacOS/" \;
    # MLX loads its Metal kernels from beside the binary first. mlx-sys
    # writes them to $MLX_RS_METAL_PATH (Deploy sets it), else to
    # ~/.mlx/lib/<mlx-c hash>/; MLX's own build leaves a copy under
    # cargo's build folder. The newest wins. Plain tests only, no pipes:
    # bash 3.2's `set -e` is unreliable around them.
    metallib=""
    for candidate in \
      "${MLX_RS_METAL_PATH:-/nonexistent}/mlx.metallib" \
      "$HOME"/.mlx/lib/*/mlx.metallib \
      $(find "$bin/build" -path '*mlx-sys-*' -name mlx.metallib 2>/dev/null || true); do
      if [ ! -f "$candidate" ]; then
        continue
      fi
      if [ -z "$metallib" ] || [ "$candidate" -nt "$metallib" ]; then
        metallib=$candidate
      fi
    done
    if [ -z "$metallib" ]; then
      echo "mlx.metallib not found; subject masks would not run. Looked in:" >&2
      echo "  \$MLX_RS_METAL_PATH (${MLX_RS_METAL_PATH:-unset}), ~/.mlx/lib, $bin/build" >&2
      ls -la "${MLX_RS_METAL_PATH:-/nonexistent}" >&2 || true
      exit 1
    fi
    echo "MLX kernels: $metallib"
    cp "$metallib" "$app/Contents/MacOS/"
    if [ "$arch" != aarch64 ]; then
      echo "no FFmpeg build is pinned for macOS $arch" >&2
      exit 1
    fi
    riedl="https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_$ffmpeg_version"
    fetch "$riedl/ffmpeg.zip" c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924
    unzip -oq "$fetched" -d "$app/Contents/MacOS"
    fetch "$riedl/ffprobe.zip" fcbe839537485eaee7a7a8bc5cbc0f90d53617e80943e8a5b2e31cb851197ea6
    unzip -oq "$fetched" -d "$app/Contents/MacOS"
    chmod +x "$app/Contents/MacOS/ffmpeg" "$app/Contents/MacOS/ffprobe"
    ffmpeg_notice "$app/Contents/Resources" \
      "Martin Riedl, https://ffmpeg.martin-riedl.de (scripts: https://git.martin-riedl.de/ffmpeg/build-script)"
    cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Flikk</string>
  <key>CFBundleDisplayName</key><string>Flikk</string>
  <key>CFBundleIdentifier</key><string>studio.flikk.Flikk</string>
  <key>CFBundleExecutable</key><string>flikk</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>${MACOSX_DEPLOYMENT_TARGET:-14.0}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
EOF
    # Ad hoc: Apple silicon runs nothing unsigned. Not notarized.
    codesign --force --deep --sign - "$app"
    archive="$name.tar.gz"
    tar -C "$stage" -czf "$dist/$archive" Flikk.app

    # The disk image: Flikk.app beside a link to /Applications. hdiutil
    # can fail with "Resource busy" on CI runners; a retry clears it.
    volume="$stage/dmg"
    mkdir -p "$volume"
    mv "$app" "$volume/"
    ln -s /Applications "$volume/Applications"
    installer="$name.dmg"
    for attempt in 1 2 3; do
      if hdiutil create -quiet -ov -fs HFS+ -format UDZO \
        -volname Flikk -srcfolder "$volume" "$dist/$installer"; then
        break
      fi
      if [ "$attempt" = 3 ]; then
        echo "hdiutil could not create $installer" >&2
        exit 1
      fi
      sleep 5
    done
    ;;
  linux)
    dir="$stage/$name"
    mkdir -p "$dir"
    cp "$bin/flikk" "$bin/flikk-mcp" "$dir/"
    # ONNX Runtime's Dawn (WebGPU), found by rpath $ORIGIN.
    find "$bin" -maxdepth 1 \( -name '*.so' -o -name '*.so.*' \) -exec cp -L {} "$dir/" \;
    if [ "$arch" != x86_64 ]; then
      echo "no FFmpeg build is pinned for Linux $arch" >&2
      exit 1
    fi
    # A month's last build: BtbN keeps those, not the daily ones. The
    # release branch, a few fixes past the tag.
    btbn="ffmpeg-n$ffmpeg_version-39-g7adfbe40dc-linux64-gpl-9.0"
    fetch "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-31-12-59/$btbn.tar.xz" \
      73e29a56974ae5a651133a04584c7483b21afe941e17dc91d79814a758b608d3
    tar -xJf "$fetched" -C "$dir" --strip-components=2 "$btbn/bin/ffmpeg" "$btbn/bin/ffprobe"
    ffmpeg_notice "$dir" "BtbN, https://github.com/BtbN/FFmpeg-Builds" \
      "https://github.com/FFmpeg/FFmpeg/commit/7adfbe40dc"
    archive="$name.tar.gz"
    tar -C "$stage" -czf "$dist/$archive" "$name"
    ;;
  windows)
    dir="$stage/$name"
    mkdir -p "$dir"
    cp "$bin/flikk.exe" "$bin/flikk-mcp.exe" "$dir/"
    find "$bin" -maxdepth 1 -name '*.dll' -exec cp {} "$dir/" \;
    gyan="ffmpeg-$ffmpeg_version-essentials_build"
    fetch "https://github.com/GyanD/codexffmpeg/releases/download/$ffmpeg_version/$gyan.7z" \
      4705843ccaaf54257c16ad90f3e952ece33c17df964ecf7bfdbb0f49c7171077
    7z e -y "-o$dir" "$fetched" "$gyan/bin/ffmpeg.exe" "$gyan/bin/ffprobe.exe" >/dev/null
    if [ ! -f "$dir/ffmpeg.exe" ] || [ ! -f "$dir/ffprobe.exe" ]; then
      echo "ffmpeg.exe or ffprobe.exe missing from $fetched" >&2
      exit 1
    fi
    ffmpeg_notice "$dir" "Gyan Doshi, https://www.gyan.dev/ffmpeg/builds/"
    archive="$name.zip"
    (cd "$stage" && 7z a -tzip "../$archive" "$name" >/dev/null)

    iscc=$(command -v iscc || command -v ISCC.exe || true)
    for candidate in "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" "/c/Program Files/Inno Setup 6/ISCC.exe"; do
      [ -z "$iscc" ] && [ -x "$candidate" ] && iscc=$candidate
    done
    if [ -n "$iscc" ]; then
      # Git Bash would rewrite the /D switches as paths. ISCC reads
      # relative paths from the .iss file's folder, so pass absolute ones.
      MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' "$iscc" /Q \
        "/DVersion=$version" \
        "/DSourceDir=$(cygpath -aw "$dir")" \
        "/DOutputDir=$(cygpath -aw "$dist")" \
        "/DOutputName=$name-setup" \
        "$(cygpath -aw "$here/windows/flikk.iss")"
      installer="$name-setup.exe"
    elif [ -n "${CI:-}" ]; then
      echo "Inno Setup (ISCC) not found; the installer can't be built" >&2
      exit 1
    else
      echo "Inno Setup (ISCC) not found; skipping the installer" >&2
    fi
    ;;
esac

rm -rf "$stage"
cd "$dist"
for file in "$archive" ${installer:+"$installer"}; do
  if command -v sha256sum >/dev/null; then
    sha256sum "$file" >"$file.sha256"
  else
    shasum -a 256 "$file" >"$file.sha256"
  fi
  echo "$dist/$file"
done
