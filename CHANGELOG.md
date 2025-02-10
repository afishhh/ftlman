## [Unreleased]

### Added

- Mod order is now automatically imported from a Slipstream modorder.txt file in the mod directory if modorder.json does not exist.
- Some timing information is now logged after applying mods.
- Syntax highlighting for regex in XML Sandbox search.

### Changed

- Some popups (like errors) are now displayed with different styling.

### Fixed

- Fixed regression introduced `v0.5.0` that made mismatched closing tags in append files an error.
- Fixed incorrect drag and drop behaviour while scrolling the mod list.
- Added error context for the opening of mod files and directories.

## [v0.5.1]

### Fixed

- Actually serialize empty XML elements as empty tags.
- Fix Lua patch thread crashes caused by insufficient or incorrect validation of node insertions.
- Fix patch thread crash when escaping comments with a `>` character.

## [v0.5.0]

### Added

- Added Lua patching API, with documentation available at <https://fishhh.dev/ftlman/lua.html>.
- `append` and `lua-run` CLI commands.
- Added search box to the XML Sandbox.
- Added controls for automatic patching to the XML Sandbox.
- Implemented support for steam installation detection on MacOS.
- MacOS builds are now provided in releases.
- Added setting to disable Hyperspace installer.
- Implemented `.xml.rawappend`/`.rawappend.xml`.

### Changed

- Made XML Sandbox remember its window size and increased the default.
- Disallow resizing either the code editor or output into a zero-width panel in the XML Sandbox.
- Made empty XML elements serialize as empty tags.
- Disallowed applying mods while the XML Sandbox is open.
- Improved localisation of the XML Sandbox.
- Switched to a custom XML parser that almost exactly replicates RapidXML's behaviour (RapidXML being the XML parser used by FTL).
- Improved XML parsing error messages in the XML sandbox.

### Fixed

- Fixed patching mods from directories on Windows.
- Fixed patching mods with some garbage top-level directories.
- Added error context to .txt file decoding error.
- Fixed XML Sandbox displaying the wrong file after the data archive's file list has been changed.

[unreleased]: https://github.com/afishhh/ftlman/compare/v0.5.1...HEAD
[v0.5.1]: https://github.com/afishhh/ftlman/compare/v0.5.0...v0.5.1
[v0.5.0]: https://github.com/afishhh/ftlman/compare/v0.4.1...v0.5.0
