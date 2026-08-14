#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(awk -F '"' '/^version = / { print $2; exit }' "$root/Cargo.toml")
# 模板：以最新一个已发布的 AppImage 为基础（内含精简 GM 音源与依赖库）
template=$(ls -1 "$root"/music_rust-*-x86_64.AppImage 2>/dev/null | grep -v -- "-${version}-" | sort -V | tail -1)
output="$root/music_rust-${version}-x86_64.AppImage"
work="$root/target/appimage-v2"
tool="$root/target/appimage/appimagetool-x86_64.AppImage"

test -n "$template" || { printf 'missing AppImage template (music_rust-*-x86_64.AppImage)\n' >&2; exit 1; }
rm -rf "$work"
mkdir -p "$work"

cargo build --release --manifest-path "$root/Cargo.toml" --bin music
(
    cd "$work"
    "$template" --appimage-extract >/dev/null
)
install -Dm755 "$root/target/release/music" "$work/squashfs-root/usr/bin/music"

if [ ! -x "$tool" ]; then
    curl --fail --location --retry 3 --output "$tool" "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
    chmod 755 "$tool"
fi

rm -f "$output" "$output.sha256"
ARCH=x86_64 "$tool" --appimage-extract-and-run "$work/squashfs-root" "$output"
(
    cd "$root"
    sha256sum "music_rust-${version}-x86_64.AppImage" \
        | awk -v v="$version" '{print $1"  music_rust-"v"-x86_64.AppImage"}' \
        > "music_rust-${version}-x86_64.AppImage.sha256"
)
