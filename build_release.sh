#!/usr/bin/env bash

set -xeuo pipefail

UNIXLIKES=(
  x86_64-unknown-linux-gnu
  x86_64-apple-darwin
  aarch64-apple-darwin
)

for target in "${UNIXLIKES[@]}"; do
  cross +nightly build --target "$target" --release
  llvm-strip --strip-all "target/$target/release/ftlman"
done

cross +nightly build -p windows_gui_wrapper --target-dir target-x86_64-pc-windows-gnu --target x86_64-pc-windows-gnu --release
cross +nightly build --target-dir target-x86_64-pc-windows-gnu --target x86_64-pc-windows-gnu --release
llvm-strip --strip-all target-x86_64-pc-windows-gnu/x86_64-pc-windows-gnu/release/*.exe

[[ -e release ]] || mkdir release
cd release

ln -f ../target-x86_64-pc-windows-gnu/x86_64-pc-windows-gnu/release/ftlman.exe ftlman.exe
ln -f ../target-x86_64-pc-windows-gnu/x86_64-pc-windows-gnu/release/windows_gui_wrapper.exe ftlman_gui.exe
7z a ftlman-x86_64-pc-windows-gnu.zip ftlman_gui.exe ftlman.exe
rm ftlman_gui.exe ftlman.exe

for target in "${UNIXLIKES[@]}"; do
  ln -f ../target/"$target"/release/ftlman ftlman
  tar cvaf "ftlman-$target.tar.gz" ftlman
  rm ftlman
done
