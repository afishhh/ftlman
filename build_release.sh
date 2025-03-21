#!/usr/bin/env bash

set -xeuo pipefail

UNIXLIKES=(
  x86_64-unknown-linux-gnu
  x86_64-apple-darwin
  aarch64-apple-darwin
)

for target in "${UNIXLIKES[@]}"; do
  cross +nightly build --features portable-release --target "$target" --release
  llvm-strip --strip-all "target/$target/release/ftlman"
done

cross +nightly build -p windows_gui_wrapper --target-dir target-x86_64-pc-windows-gnu  --target x86_64-pc-windows-gnu --release
cross +nightly build --target-dir target-x86_64-pc-windows-gnu --features portable-release --target x86_64-pc-windows-gnu --release
llvm-strip --strip-all target-x86_64-pc-windows-gnu/x86_64-pc-windows-gnu/release/*.exe

[[ -e release ]] || mkdir release
cd release

mkdir ftlman
cd ftlman
mkdir mods
ln -f ../../target-x86_64-pc-windows-gnu/x86_64-pc-windows-gnu/release/ftlman.exe ftlman.exe
ln -f ../../target-x86_64-pc-windows-gnu/x86_64-pc-windows-gnu/release/windows_gui_wrapper.exe ftlman_gui.exe
cd ..
7z a ftlman-x86_64-pc-windows-gnu.zip ftlman
rm -r ftlman

for target in "${UNIXLIKES[@]}"; do
  mkdir -p ftlman/mods
  cd ftlman
  ln -f ../../target/"$target"/release/ftlman ftlman
  cd ..
  tar cvaf "ftlman-$target.tar.gz" ftlman
  rm -r ftlman
done
