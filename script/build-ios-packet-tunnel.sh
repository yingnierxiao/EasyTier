#!/usr/bin/env bash
set -euo pipefail

# 扩展只链接核心静态库，界面的 Tauri 运行时不进入扩展进程。
EASYTIER_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EASYTIER_ARCH="${ARCHS:-arm64}"
EASYTIER_ARCH="${EASYTIER_ARCH%% *}"
case "${PLATFORM_NAME:-iphoneos}:$EASYTIER_ARCH" in
    iphoneos:arm64) EASYTIER_TARGET=aarch64-apple-ios ;;
    iphonesimulator:arm64) EASYTIER_TARGET=aarch64-apple-ios-sim ;;
    iphonesimulator:x86_64) EASYTIER_TARGET=x86_64-apple-ios ;;
    *) echo "Unsupported iOS platform or architecture" >&2; exit 1 ;;
esac

export CARGO_TARGET_DIR="$EASYTIER_REPO_ROOT/target/ios-packet-tunnel"
export CARGO_PROFILE_RELEASE_LTO=thin
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
export CARGO_PROFILE_RELEASE_STRIP=debuginfo
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export IPHONEOS_DEPLOYMENT_TARGET=15.0
if [[ "${PLATFORM_NAME:-iphoneos}" == iphonesimulator ]]; then
    EASYTIER_SIM_ARCH=arm64
    [[ "$EASYTIER_ARCH" != x86_64 ]] || EASYTIER_SIM_ARCH=x86_64
    export BINDGEN_EXTRA_CLANG_ARGS="--target=$EASYTIER_SIM_ARCH-apple-ios15.0-simulator -isysroot ${SDKROOT:?}"
fi
cd "$EASYTIER_REPO_ROOT"
cargo build --locked -p easytier-ios --release --target "$EASYTIER_TARGET"
EASYTIER_OUTPUT="$EASYTIER_REPO_ROOT/easytier-gui/src-tauri/gen/apple/Externals/packet-tunnel/${PLATFORM_NAME:-iphoneos}"
mkdir -p "$EASYTIER_OUTPUT"
cp "$CARGO_TARGET_DIR/$EASYTIER_TARGET/release/libeasytier_ios.a" "$EASYTIER_OUTPUT/"
