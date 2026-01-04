#!/usr/bin/env bash

set -xeuo pipefail

UNIXLIKES=(
  x86_64-unknown-linux-gnu
  x86_64-apple-darwin
  aarch64-apple-darwin
)

wanted_target=${1:-}

for target in "${UNIXLIKES[@]}"; do
  continue=0
  if [[ -n $wanted_target && $target != $wanted_target ]]; then
    continue=1
  fi
  if [[ $continue == 0 ]]; then
    cross +nightly build --features portable-release --target "$target" --release
    llvm-strip --strip-all "target/$target/release/ftlman"
  fi
done

[[ -e release ]] || mkdir release
cd release

if [[  -z $wanted_target || "x86_64-pc-windows-gnu" == $wanted_target ]]; then
  cross +nightly build --target-dir ../target-x86_64-pc-windows-gnu --features portable-release --target x86_64-pc-windows-gnu --release
  llvm-strip --strip-all ../target-x86_64-pc-windows-gnu/x86_64-pc-windows-gnu/release/*.exe

  mkdir -p ftlman/mods
  cd ftlman
  ln -f ../../target-x86_64-pc-windows-gnu/x86_64-pc-windows-gnu/release/ftlman.exe ftlman.exe
  cd ..
  7zz a ftlman-x86_64-pc-windows-gnu.zip ftlman
  rm -r ftlman
fi

for target in "${UNIXLIKES[@]}"; do
  continue=0
  if [[ -n $wanted_target && $target != $wanted_target ]]; then
    continue=1
  fi
  if [[ $continue == 0 ]]; then
    mkdir -p ftlman/mods
    cd ftlman
    ln -f ../../target/"$target"/release/ftlman ftlman
    cd ..
    tar cvaf "ftlman-$target.tar.gz" ftlman
    rm -r ftlman
  fi
done
