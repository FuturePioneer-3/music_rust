#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(awk -F '"' '/^version = / { print $2; exit }' "$root/Cargo.toml")
version=${version%.0}   # Cargo 使用完整 semver，发布文件名使用简化版本号
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

# TXT v3.2 的图片解压由 Rust 直接调用系统库；把运行库一起放入 AppImage，
# 避免目标机器必须预装这些动态库。
for lib in libzstd.so.1 libz.so.1 libbz2.so.1.0 liblzma.so.5 liblz4.so.1; do
    if [ -f "$work/squashfs-root/usr/lib/$lib" ]; then
        continue
    fi
    host_lib=$(ldconfig -p 2>/dev/null | awk -v l="$lib" '$1 == l {print $NF; exit}')
    if [ -n "$host_lib" ] && [ -f "$host_lib" ]; then
        install -Dm755 "$host_lib" "$work/squashfs-root/usr/lib/$lib"
    else
        printf 'missing runtime library: %s\n' "$lib" >&2
        exit 1
    fi
done

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
