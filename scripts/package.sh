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
#
# The updater (crates/flikk/src/update.rs, `asset_name`) looks for these
# names, and lays the archive's one folder over the installed copy.
set -euo pipefail

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
    # MLX loads its Metal kernels from beside the binary first.
    metallib=$(
      for dir in "${MLX_RS_METAL_PATH:-}" "$bin/build" "$HOME/.mlx"; do
        [ -n "$dir" ] && [ -e "$dir" ] && find "$dir" -name mlx.metallib 2>/dev/null
      done | head -n 1
    )
    if [ -z "$metallib" ]; then
      echo "mlx.metallib not found; subject masks would not run" >&2
      exit 1
    fi
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
    ;;
esac

rm -rf "$stage"
cd "$dist"
if command -v sha256sum >/dev/null; then
  sha256sum "$archive" >"$archive.sha256"
else
  shasum -a 256 "$archive" >"$archive.sha256"
fi
echo "$dist/$archive"
