#!/usr/bin/env bash

set -euo pipefail

assets="Multiverse Assets.zip"
data="Multiverse Data.zip"

# Assets (5.4.5)
[[ -e $assets ]] ||
	./google_drive_download.py -o "$assets" 17TtZlTNmMKTC4DtXl1N5cK8MA2OYQPNs
# Data (5.4.6)
[[ -e $assets ]] ||
	./google_drive_download.py -o "$data" 1C3lc3foHn_iP5RUp_mvOirGMJKLYoNpF

ln -sf ../../"Multiverse Assets.zip" slipstream/mods/
ln -sf ../../"Multiverse Data.zip" slipstream/mods/
