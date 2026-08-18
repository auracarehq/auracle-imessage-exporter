#!/bin/sh
set -eu

artifact_dir=${AURACLE_ARTIFACT_DIR:-dist}
binary_name=auracle-imessage-exporter
rust_toolchain=${AURACLE_RUST_TOOLCHAIN:-stable}
signing_mode=signed
if [ "${1:-}" = "--unsigned" ]; then
    signing_mode=unsigned
elif [ "$#" -ne 0 ]; then
    echo "usage: scripts/build-universal.sh [--unsigned]" >&2
    exit 2
fi

export CARGO_INCREMENTAL=0
if command -v git >/dev/null 2>&1; then
    SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git show -s --format=%ct HEAD)}
    export SOURCE_DATE_EPOCH
fi

rustup target add --toolchain "$rust_toolchain" aarch64-apple-darwin x86_64-apple-darwin
release_cargo=$(rustup which --toolchain "$rust_toolchain" cargo)
release_rustc=$(rustup which --toolchain "$rust_toolchain" rustc)
RUSTC="$release_rustc" "$release_cargo" build --locked --release -p imessage-exporter \
    --bin "$binary_name" --target aarch64-apple-darwin
RUSTC="$release_rustc" "$release_cargo" build --locked --release -p imessage-exporter \
    --bin "$binary_name" --target x86_64-apple-darwin

mkdir -p "$artifact_dir"
lipo -create \
    "target/aarch64-apple-darwin/release/$binary_name" \
    "target/x86_64-apple-darwin/release/$binary_name" \
    -output "$artifact_dir/$binary_name"
chmod 755 "$artifact_dir/$binary_name"

if [ "$signing_mode" = signed ]; then
    code_sign_identity=${AURACLE_CODESIGN_IDENTITY:?Set AURACLE_CODESIGN_IDENTITY to a Developer ID Application identity}
    codesign --force --options runtime --timestamp \
        --sign "$code_sign_identity" "$artifact_dir/$binary_name"
    codesign --verify --deep --strict --verbose=2 "$artifact_dir/$binary_name"
fi

(
    cd "$artifact_dir"
    shasum -a 256 "$binary_name" > "$binary_name.sha256"
)
lipo -archs "$artifact_dir/$binary_name"
(
    cd "$artifact_dir"
    shasum -a 256 -c "$binary_name.sha256"
)
