#!/usr/bin/env bash
# Package a release build of Flikk for this platform into dist/, with a
# `sha256sum` line beside it:
#
#   cargo build --release --locked -p flikk -p flikk-agent
#   scripts/package.sh 0.2.0
#
#   macOS    dist/flikk-0.2.0-macos-aarch64.tar.gz    Flikk.app
#   Linux    dist/flikk-0.2.0-linux-x86_64.tar.gz     flikk-0.2.0-linux-x86_64/
#   Windows  dist/flikk-0.2.0-windows-x86_64.zip      flikk-0.2.0-windows-x86_64/
#            dist/flikk-0.2.0-windows-x86_64-setup.exe, a per-user installer
#            (scripts/windows/flikk.iss) built with Inno Setup 6, when
#            ISCC is installed; required on CI
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
    ;;
  linux)
    dir="$stage/$name"
    mkdir -p "$dir"
    cp "$bin/flikk" "$bin/flikk-mcp" "$dir/"
    # ONNX Runtime's Dawn (WebGPU), found by rpath $ORIGIN.
    find "$bin" -maxdepth 1 \( -name '*.so' -o -name '*.so.*' \) -exec cp -L {} "$dir/" \;
    archive="$name.tar.gz"
    tar -C "$stage" -czf "$dist/$archive" "$name"
    ;;
  windows)
    dir="$stage/$name"
    mkdir -p "$dir"
    cp "$bin/flikk.exe" "$bin/flikk-mcp.exe" "$dir/"
    find "$bin" -maxdepth 1 -name '*.dll' -exec cp {} "$dir/" \;
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
        "$(cygpath -aw scripts/windows/flikk.iss)"
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
