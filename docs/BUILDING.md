# Reproducible builds

## Prerequisites

- Xcode Command Line Tools for the macOS SDK, `codesign`, and `lipo`
- Rust 1.97.0
- Apple targets `aarch64-apple-darwin` and `x86_64-apple-darwin`

Use the exact source tag and committed lockfile:

```sh
git checkout v1.0.0
rustup toolchain install 1.97.0 --profile minimal
rustup target add --toolchain 1.97.0 aarch64-apple-darwin x86_64-apple-darwin
```

Build each architecture without incremental state and combine them:

```sh
export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)"
release_cargo="$(rustup which --toolchain 1.97.0 cargo)"
release_rustc="$(rustup which --toolchain 1.97.0 rustc)"
RUSTC="$release_rustc" "$release_cargo" build --locked --release -p imessage-exporter \
  --bin auracle-imessage-exporter --target aarch64-apple-darwin
RUSTC="$release_rustc" "$release_cargo" build --locked --release -p imessage-exporter \
  --bin auracle-imessage-exporter --target x86_64-apple-darwin
mkdir -p dist
lipo -create \
  target/aarch64-apple-darwin/release/auracle-imessage-exporter \
  target/x86_64-apple-darwin/release/auracle-imessage-exporter \
  -output dist/auracle-imessage-exporter
lipo -archs dist/auracle-imessage-exporter
```

Run `AURACLE_RUST_TOOLCHAIN=1.97.0 scripts/build-universal.sh --unsigned` for
the same unsigned build.
Compare SHA-256 digests only when the Rust version, source commit, Cargo.lock,
macOS SDK, linker, and build environment match. Developer ID timestamps make
the final signed bytes intentionally non-reproducible; verify those with the
code signature and published checksum instead.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
file dist/auracle-imessage-exporter
lipo -archs dist/auracle-imessage-exporter
```
