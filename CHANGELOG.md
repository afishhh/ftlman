## [Unreleased]

### Added

- `mod.meta.current_path` field for getting the path of the currently executing Lua script.
- `mod.util.eval` now accepts an additional option `path` that allows you to set the path that will be returned by
  `mod.meta.current_path` inside the evaluated code. If this option is not provided `mod.meta.current_path` will return nil.
- If an XML parse error occurs during patching, a diagnostic will be emitted for it.

### Changed

- **Breaking change**: `mod.debug.pretty_print` and `mod.debug.pretty_string` no longer accept unknown fields in their options.
  This was never intended to be allowed and allows for future compatible additions of more options.

### Fixed

- **Breaking change**: `mod:insertByFind` tags with unknown tags as children now properly make patching fail instead of being ignored.
  This was a regression introduced in v0.6.0 and is now being fixed.

## [0.6.0]

### Added

- XML append script syntax errors are now reported using the diagnostics system like in the sandbox.
- Relative paths are now allowed in the mod directory field in settings. (will be relative to the `ftlman` executable's parent directory)

### Changed

- Release archives now contain a more Slipstream-like directory structure, which is
  now used instead of the global `mods` and `settings.json` set up by default previously.
  If you already have the global setup in place, you will be asked whether you wish
  to migrate to the new setup or keep using the existing global state.

### Fixed

- FTL autodetection via Steam on Windows, previously string unescaping would yield an incorrect path and prevent the detection from working.

## [v0.5.4]

### Added

- `mod.util.eval` function for evaluating Lua code from Lua.
- `mod.xml.append` function for executing XML append files from Lua.
- `Node:clone` function for cloning XML nodes in Lua.
- Flat variants of existing colorschemes that disable all rounded corners.
- The mod manager will automatically check for and notify you of updates.

### Changed

- Lua chunks are now given better names which means Lua stack traces will be slightly cleaner.
- Moved some settings under an "Advanced settings" collapsible section.

### Fixed

- Unclosed tags are now allowed in mod XML (fixes compatibility with some mods).

## [v0.5.3]

### Fixed

- Line endings of .txt files are now normalized to CRLF.
- Lua XML DOM operations will now ignore unsupported nodes instead of triggering a panic.

## [v0.5.2]

### Added

- Mod order is now automatically imported from a Slipstream `modorder.txt` file in the mod directory if `modorder.json` does not exist.
- Some timing information is now logged after applying mods.
- Syntax highlighting for regex in XML Sandbox search.

### Changed

- Some popups (like errors) are now displayed with different styling.

### Fixed

- Fixed regression introduced in `v0.5.0` that made mismatched closing tags in append files an error.
- Fixed incorrect drag and drop behaviour while scrolling the mod list.
- Added error context for the opening of mod files and directories.
- Fixed handling of `\` directory separators in non-standard zip files.

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

[unreleased]: https://github.com/afishhh/ftlman/compare/v0.6.0...HEAD
[v0.6.0]: https://github.com/afishhh/ftlman/compare/v0.5.4...v0.6.0
[v0.5.4]: https://github.com/afishhh/ftlman/compare/v0.5.3...v0.5.4
[v0.5.3]: https://github.com/afishhh/ftlman/compare/v0.5.2...v0.5.3
[v0.5.2]: https://github.com/afishhh/ftlman/compare/v0.5.1...v0.5.2
[v0.5.1]: https://github.com/afishhh/ftlman/compare/v0.5.0...v0.5.1
[v0.5.0]: https://github.com/afishhh/ftlman/compare/v0.4.1...v0.5.0
