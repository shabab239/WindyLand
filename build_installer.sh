#!/usr/bin/env bash
# Cross-compile WindyLand for Windows and build the installer (needs rustup, gcc-mingw-w64-x86-64, nsis).
set -euo pipefail
cd "$(dirname "$0")"
rustup target add x86_64-pc-windows-gnu
mkdir -p .cargo dist
[ -f .cargo/config.toml ] || printf '[target.x86_64-pc-windows-gnu]\nlinker = "x86_64-w64-mingw32-gcc"\n' > .cargo/config.toml
cargo build --release --target x86_64-pc-windows-gnu
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
makensis -V2 "-DVERSION=$VERSION" "-DOUTDIR=$PWD/dist" "-DEXE=$PWD/target/x86_64-pc-windows-gnu/release/windyland.exe" installer/WindyLandSetup.nsi
echo "Installer: dist/WindyLandTobiSetup-$VERSION.exe"
