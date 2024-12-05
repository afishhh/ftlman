#!/usr/bin/env bash

set -xeuo pipefail

cross +nightly build --target x86_64-unknown-linux-gnu --release
cross +nightly build -p windows_gui_wrapper --target x86_64-pc-windows-gnu --release
cross +nightly build --target x86_64-pc-windows-gnu --release

[[ -e release ]] || mkdir release
cd release

ln ../target/x86_64-pc-windows-gnu/release/ftlman.exe ftlman.com
ln ../target/x86_64-pc-windows-gnu/release/windows_gui_wrapper.exe ftlman.exe
7z a ftlman-x86_64-pc-windows-gnu.zip ftlman.com ftlman.exe
rm ftlman.com ftlman.exe

ln ../target/x86_64-unknown-linux-gnu/release/ftlman ftlman
tar cvaf ftlman-x86_64-unknown-linux-gnu.tar.gz ftlman
rm ftlman
