## [Unreleased]

### Added

- `allow_top_level_text` option to Lua `mod.xml.parse` (defaults to `false` as that is the current behaviour).

### Changed

- The executable directory itself is now also considered an FTL directory candidate, although such installation layout is discouraged.

### Fixed

- Humble/GOG 1.6.12 Linux installations are now correctly recognized as not needing a downgrade.

## [v0.6.4]

### Added

- Dragging and dropping a mod file onto ftlman will now move/copy the file into your mod directory automatically.

### Changed

- Improved error message on invalid mod zip archive.
- Removed `ftlman_gui.exe` wrapper since it triggers AVs and causes unnecessary confusion.

### Fixed

- Fixed a potential crash when changing mod list filter related settings (like "Treat zips as mods").
- Scanning now adheres to the current in-memory mod order instead of unconditionally preferring the modorder.json file. (https://github.com/afishhh/ftlman/issues/3)

## [v0.6.3]

### Added

- Find tag panics can now specify a custom message by using `panic="message here"`. Note that the values "true" and "false" are still treated like they were previously.
- Added an automatic update installer, you will now have the option of having ftlman updates installed for you automatically.
- The parent of ftlman's executable directory is now also considered a candidate for the FTL installation directory during autodetection.

### Changed

- Find tag panics now emit a friendly diagnostic instead of displaying the `Debug` representation of the internal `Find` structure.

### Fixed

- Hopefully fixed some antivirus false positives on `ftlman_gui.exe`.
- Context is now attached more consistently to errors that occur when reading files from a mod.

## [v0.6.2]

### Added

- Subcommand for downloading and installing Hyperspace via the CLI.
- More FTL versions are now recognized correctly.

### Fixed

- File paths of append base files are now matched case-insensitively during patching.
- Worked around assertion failure on startup when linking with a fontconfig compiled with assertions enabled on Linux.
- The `patch` subcommand now works correctly with relative mod directories.

## [v0.6.1]

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
- Unclosed XML elements in `.xml` files will now be properly closed during patching.

## [v0.6.0]

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

[unreleased]: https://github.com/afishhh/ftlman/compare/v0.6.4...HEAD
[v0.6.4]: https://github.com/afishhh/ftlman/compare/v0.6.3...v0.6.4
[v0.6.3]: https://github.com/afishhh/ftlman/compare/v0.6.2...v0.6.3
[v0.6.2]: https://github.com/afishhh/ftlman/compare/v0.6.1...v0.6.2
[v0.6.1]: https://github.com/afishhh/ftlman/compare/v0.6.0...v0.6.1
[v0.6.0]: https://github.com/afishhh/ftlman/compare/v0.5.4...v0.6.0
[v0.5.4]: https://github.com/afishhh/ftlman/compare/v0.5.3...v0.5.4
[v0.5.3]: https://github.com/afishhh/ftlman/compare/v0.5.2...v0.5.3
[v0.5.2]: https://github.com/afishhh/ftlman/compare/v0.5.1...v0.5.2
[v0.5.1]: https://github.com/afishhh/ftlman/compare/v0.5.0...v0.5.1
[v0.5.0]: https://github.com/afishhh/ftlman/compare/v0.4.1...v0.5.0
