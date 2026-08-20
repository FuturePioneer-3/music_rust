#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(awk -F '"' '/^version = / { print $2; exit }' "$root/Cargo.toml")
version=${version%.0}   # 3.1.0 → 3.1（对外发布名）
# 模板：取目录中版本号最大的现有 AppImage（排除当前版本自身）
template=$(ls -1 "$root"/music_rust-*-x86_64.AppImage 2>/dev/null | grep -Fv -- "-${version}-" | sort -V | tail -1)
output="$root/music_rust-${version}-x86_64.AppImage"
work="$root/target/appimage-v2"
tool="$root/target/appimage/appimagetool-x86_64.AppImage"

test -f "$template" || { printf 'missing AppImage template: %s\n' "$template" >&2; exit 1; }
test "$(basename "$template")" = "$(basename "$output")" && { printf 'template is the same file as output: %s\n' "$template" >&2; exit 1; }
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
