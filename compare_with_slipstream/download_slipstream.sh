#!/usr/bin/env bash

set -euo pipefail

[[ -e slipstream ]] ||
	curl -L https://sourceforge.net/projects/slipstreammodmanager/files/Slipstream/1.9.1/SlipstreamModManager_1.9.1-Unix.tar.gz/download |
		tar xvaz --one-top-level=slipstream --strip-components=1

ln -sf ../modman.cfg slipstream/
