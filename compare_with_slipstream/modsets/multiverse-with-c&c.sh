#!/usr/bin/env bash

source modsets/multiverse.sh

download_mod wget "Vertex Util" https://github.com/ChronoVortex/FTL-HS-Vertex/releases/download/v6.3/Vertex-Util.ftl
download_mod wget "Multiverse Tiberian Sun Pack" https://github.com/ChronoVortex/FTL-Multiverse-Tiberian-Sun/releases/download/v10.3/Tiberian_Sun_Pack_MV.ftl
download_mod wget "Multiverse C&C Weapons" https://github.com/ChronoVortex/FTL-Multiverse-CnC-Weapons/releases/download/v4.6/CNC_Weapons_Renegade.ftl

export PATCHED_SLIPSTREAM_HASH=ddea635113a904197c8ddb172804bd551a344ff30fb975c36322c883534fdc13
