#!/bin/sh
set -eu

archive=$1
output=$2
symbols="$output.symbols"

# Ghostty and Skia bundle different Wuffs versions. Rename both the
# definitions and their references throughout Ghostty's archive.
llvm_bin=
if ! command -v llvm-objcopy >/dev/null 2>&1 || ! command -v llvm-nm >/dev/null 2>&1; then
    if command -v brew >/dev/null 2>&1; then
        llvm_bin="$(brew --prefix llvm)/bin/"
    fi
fi
if ! command -v "${llvm_bin}llvm-objcopy" >/dev/null 2>&1 || ! command -v "${llvm_bin}llvm-nm" >/dev/null 2>&1; then
    echo "The Ghostty static library requires llvm-nm and llvm-objcopy" >&2
    exit 1
fi

"${llvm_bin}llvm-nm" --arch=all --extern-only --defined-only --just-symbol-name "$archive" > "$symbols"
awk '
    /^_?(wuffs_|sizeof__wuffs_)/ {
        if (seen[$0]++) next
        if ($0 ~ /^_/) print $0, "_craic_vt" $0
        else print $0, "craic_vt_" $0
    }
' "$symbols" > "$symbols.map"
if [ -s "$symbols.map" ]; then
    "${llvm_bin}llvm-objcopy" --redefine-syms="$symbols.map" "$archive" "$output"
else
    cp "$archive" "$output"
fi
rm "$symbols" "$symbols.map"
