#!/bin/zsh
set -eu
repo="$(cd "$(dirname "$0")" && pwd)"
cd "$repo"
cargo_target_dir="${CARGO_TARGET_DIR:-$repo/target}"
CARGO_TARGET_DIR="$cargo_target_dir" cargo build --release --locked
destination="${1:-$repo/../Pixy Control.app}"
mkdir -p "$destination/Contents/MacOS" "$destination/Contents/Resources"
cp assets/Info.plist "$destination/Contents/Info.plist"
cp "$cargo_target_dir/release/pixy-control" "$destination/Contents/MacOS/Pixy Control"
codesign --force --sign - "$destination"
