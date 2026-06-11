#!/bin/bash
set -e

# Build the av-probe binary in release mode
echo "Building av-probe release binary..."
cargo build --release -p av-probe

echo "Build complete. Executable is located at: target/release/av-probe"
ls -lh target/release/av-probe
