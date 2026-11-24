#!/bin/sh
set -eu

prefix=$1
target=$2
version=1.0.1
zig_version=0.17.0
stamp="$prefix/.built-$version-$target"
[ ! -f "$stamp" ] || exit 0

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
download() {
    curl --fail --silent --show-error --location --retry 3 "$1" -o "$3"
    if command -v sha256sum >/dev/null 2>&1; then
        printf '%s  %s\n' "$2" "$3" | sha256sum -c -
    else
        printf '%s  %s\n' "$2" "$3" | shasum -a 256 -c -
    fi
}

release="https://release.files.ghostty.org/libghostty-vt/$version"
case "$target" in
    aarch64-apple-darwin|x86_64-apple-darwin)
        download "$release/ghostty-vt-$version.xcframework.zip" \
            c0f9631277f726154c8f521954d17cdc41aac4626fc0df8cf925a19ee58e829b "$work/apple.zip"
        unzip -q "$work/apple.zip" -d "$work"
        apple="$work/ghostty-vt.xcframework/macos-arm64_x86_64"
        mkdir -p "$prefix/lib" "$prefix/include"
        cp "$apple/libghostty-vt.a" "$prefix/lib/"
        cp -R "$apple/Headers/ghostty" "$prefix/include/"
        ;;
    *-linux-*)
        case "$target" in
            x86_64-*) arch=x86_64 ;;
            aarch64-*) arch=aarch64 ;;
            *) echo "Set GHOSTTY_VT_DIR to libghostty-vt for $target" >&2; exit 1 ;;
        esac
        case "$target" in
            *-linux-gnu) abi=gnu ;;
            *-linux-musl) abi=musl ;;
            *) echo "Set GHOSTTY_VT_DIR to libghostty-vt for $target" >&2; exit 1 ;;
        esac
        zig=${ZIG:-zig}
        case "$("$zig" version 2>/dev/null || true)" in
            0.17.[0-9]*) ;;
            *)
                case "$(uname -sm)" in
                    'Linux x86_64') host=x86_64-linux; hash=1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026 ;;
                    'Linux aarch64') host=aarch64-linux; hash=9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8 ;;
                    *) echo "Set ZIG to Zig 0.17 or set GHOSTTY_VT_DIR" >&2; exit 1 ;;
                esac
                download "https://ziglang.org/download/$zig_version/zig-$host-$zig_version.tar.xz" \
                    "$hash" "$work/zig.tar.xz"
                tar -xJf "$work/zig.tar.xz" -C "$work"
                zig="$work/zig-$host-$zig_version/zig"
                ;;
        esac
        source="libghostty-vt-$version"
        download "$release/$source.tar.gz" \
            333dfce97641ef9b536bb9e08907ad86abaaada50e8f7417b70d9284adef37aa "$work/source.tar.gz"
        download "$release/$source-deps.tar.gz" \
            91a4e10960a52bf5bf4b15aa2a2cddaff8a89d6cd903d3055bda4fd17de3356d "$work/deps.tar.gz"
        tar -xzf "$work/source.tar.gz" -C "$work"
        tar -xzf "$work/deps.tar.gz" -C "$work"
        (
            cd "$work/$source"
            ZIG_GLOBAL_CACHE_DIR="$work/cache" "$zig" build --prefix "$prefix" \
                --system "$work/$source-deps" -Dtarget="$arch-linux-$abi" -Dcpu=baseline -Doptimize=ReleaseFast
        )
        ;;
    *) echo "Set GHOSTTY_VT_DIR to libghostty-vt for $target" >&2; exit 1 ;;
esac
touch "$stamp"
