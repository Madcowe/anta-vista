#!/bin/bash
#
# Cross-compile the `av` binary (av-cli) for Apple Silicon macOS from Linux
# using cargo-zigbuild + zig.
#
# Usage:
#   scripts/build-macos.sh
#
# Environment overrides:
#   ZIG_VERSION   zig release to bootstrap (default: 0.17.0)
#   MACOS_SDK     path to a real macOS SDK to link against instead of the
#                 generated stub (exported as SDKROOT)
#
# How the SDK problem is solved:
#   ort-sys emits `-framework Foundation` unconditionally on Apple targets,
#   and zig ships no framework stubs, so a bare link fails with
#   "unable to find system framework". Rather than requiring an Apple-licensed
#   SDK, the script generates a minimal Text API stub (Foundation.tbd) listing
#   the two symbols the final link actually binds to Foundation
#   (___CFConstantStringClassReference via CoreFoundation, which the real
#   Foundation re-exports, and _NSLog) — the same technique cargo-zigbuild
#   itself uses for libiconv. The binary's dylib table then matches what a
#   natively linked binary records, and dyld on a real Mac resolves both
#   through the real framework. Point MACOS_SDK at a full SDK to skip the stub.
#
# Other deliberate choices:
#   - cargo-zigbuild uses zig's own ar/ranlib unconditionally (the old
#     --zig-ar flag is gone), so Mach-O archives from libsqlite3-sys /
#     onig_sys are indexed correctly.
#   - Only av-cli is built: av-probe pulls in extra macOS frameworks
#     (CoreFoundation via chrono, libdispatch via ctrlc) that increase link
#     risk and are not needed for the shipped binary.
#   - lld64 embeds an ad-hoc code signature automatically for arm64; if a
#     Mac still refuses to run the binary, `codesign -s - av` on the Mac
#     re-signs it.
#   - The binary cannot be executed on this machine; verification is a
#     clean link plus `file` reporting a Mach-O arm64 executable.
set -euo pipefail

TARGET="aarch64-apple-darwin"
ZIG_VERSION="${ZIG_VERSION:-0.17.0}"
MACOSX_DEPLOYMENT_TARGET="11.0"

cd "$(dirname "$0")/.."

# --- bootstrap: rustup target ----------------------------------------------
if ! rustup target list --installed | grep -qx "$TARGET"; then
    echo "Installing rustup target $TARGET..."
    rustup target add "$TARGET"
fi

# --- bootstrap: cargo-zigbuild ---------------------------------------------
# Note: `cargo zigbuild --version` is not supported; --help is the reliable probe.
if ! cargo zigbuild --help >/dev/null 2>&1; then
    echo "Installing cargo-zigbuild..."
    cargo install --locked cargo-zigbuild
fi

# --- bootstrap: zig ---------------------------------------------------------
# No pip on this box, so fetch the official tarball into the user cache.
ZIG_CACHE="$HOME/.cache/anta-vista/zig"
ZIG_DIR="$ZIG_CACHE/zig-x86_64-linux-$ZIG_VERSION"
if [ ! -x "$ZIG_DIR/zig" ]; then
    echo "Downloading zig $ZIG_VERSION..."
    mkdir -p "$ZIG_CACHE"
    tarball="zig-x86_64-linux-$ZIG_VERSION.tar.xz"
    curl -fL -o "$ZIG_CACHE/$tarball" \
        "https://ziglang.org/download/$ZIG_VERSION/$tarball"
    tar -xJf "$ZIG_CACHE/$tarball" -C "$ZIG_CACHE"
fi
export PATH="$ZIG_DIR:$PATH"
echo "Using zig $(zig version)"

# --- SDK: real one if given, otherwise generate a stub ----------------------
if [ -n "${MACOS_SDK:-}" ]; then
    export SDKROOT="$MACOS_SDK"
    echo "Using macOS SDK at $SDKROOT"
else
    SDKROOT="$HOME/.cache/anta-vista/macos-sdk-stub"
    if [ ! -f "$SDKROOT/System/Library/Frameworks/Foundation.framework/Foundation.tbd" ]; then
        echo "Generating stub macOS SDK at $SDKROOT..."
        mkdir -p "$SDKROOT/System/Library/Frameworks/Foundation.framework"
        mkdir -p "$SDKROOT/usr/include" "$SDKROOT/usr/lib"
        cat > "$SDKROOT/System/Library/Frameworks/Foundation.framework/Foundation.tbd" <<'TBD'
--- !tapi-tbd
tbd-version:     4
targets:         [ arm64-macos ]
install-name:    '/System/Library/Frameworks/Foundation.framework/Versions/C/Foundation'
current-version: 1775.118.101
compatibility-version: 300
exports:
  - targets:         [ arm64-macos ]
    symbols:         [ ___CFConstantStringClassReference, _NSLog,
                       _NSTemporaryDirectory ]
TBD
    fi
    export SDKROOT
    echo "Using stub macOS SDK at $SDKROOT (set MACOS_SDK for a real one)"
fi

# --- build ------------------------------------------------------------------
echo "Building av release binary for $TARGET..."
set +e
MACOSX_DEPLOYMENT_TARGET="$MACOSX_DEPLOYMENT_TARGET" \
    cargo zigbuild --release -p av-cli --target "$TARGET"
status=$?
set -e

if [ "$status" -ne 0 ]; then
    echo
    echo "Build failed."
    if [ -z "${MACOS_SDK:-}" ]; then
        echo "If the error mentions a framework other than Foundation (e.g."
        echo "'-framework CoreFoundation'), the generated stub only covers"
        echo "Foundation. Either extend the stub's Foundation.tbd exports, or"
        echo "retry with a real SDK:"
        echo "  MACOS_SDK=/path/to/MacOSX.sdk scripts/build-macos.sh"
        echo "On a Mac: xcrun --sdk macosx --show-sdk-path"
    fi
    exit "$status"
fi

# --- verify ------------------------------------------------------------------
BIN="target/$TARGET/release/av"
echo
file "$BIN"
ls -lh "$BIN"
