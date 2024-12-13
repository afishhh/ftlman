#!/usr/bin/env bash

set -xeuo pipefail

cross +nightly build --target x86_64-unknown-linux-gnu --release
cross +nightly build -p windows_gui_wrapper --target x86_64-pc-windows-gnu --release
cross +nightly build --target x86_64-pc-windows-gnu --release

[[ -e release ]] || mkdir release
cd release

ln ../target/x86_64-pc-windows-gnu/release/ftlman.exe ftlman.exe
ln ../target/x86_64-pc-windows-gnu/release/windows_gui_wrapper.exe ftlman_gui.exe
7z a ftlman-x86_64-pc-windows-gnu.zip ftlman_gui.exe ftlman.exe
rm ftlman_gui.exe ftlman.exe

ln ../target/x86_64-unknown-linux-gnu/release/ftlman ftlman
tar cvaf ftlman-x86_64-unknown-linux-gnu.tar.gz ftlman
rm ftlman
