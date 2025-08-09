name = FTL Manager v{$version}

state-yes = Yes
state-no = No

mods-title = Mods
mods-unselect-all = Unselect all
mods-select-all = Select all
mods-apply-button = Apply
mods-apply-tooltip = Apply mods to FTL
mods-scan-button = Scan
mods-scan-tooltip = Rescan mod folder

status-preparing = Preparing
status-hyperspace-download = Downloading Hyperspace
status-hyperspace-download2 = Downloading Hyperspace {$version} ({$done}/{$total})
status-patch-download = Downloading game patch
status-patch-download2 = Downloading patch for {$version} ({$done}/{$total})
status-hyperspace-install = Installing Hyperspace
status-applying-mod = Applying {$mod}
status-repacking = Repacking archive
status-scanning-mods = Scanning mod folder

invalid-ftl-directory = Invalid FTL directory specified
hyperspace-fetch-releases-failed = Failed to fetch hyperspace releases

hyperspace = Hyperspace
hyperspace-releases-loading = Loading...
hyperspace-fetching-releases = Fetching hyperspace releases...

mod-meta-authors = Authors:
mod-meta-hs-req = Required hyperspace version:
mod-meta-hs-req-fallback = Requires hyperspace
mod-meta-hs-overwrites = Overwrites hyperspace.xml:
mod-meta-none = No metadata available for this mod
mod-meta-hint = Hover over a mod and its description will appear here.

pathedit-tooltip =
    Use Tab and Shift+Tab to cycle suggestions
    Press Enter to accept a suggestion

findftl-failed-title = FTL directory autodetection failed

sandbox-button = XML Sandbox
sandbox-title = {sandbox-button}
sandbox-open-failed = Failed to open XML Sandbox
sandbox-editor-hint-xml-append = Type XML append code here to apply it to the selected file
sandbox-editor-hint-lua-append = Type Lua append code here to apply it to the selected file
sandbox-mode-label = Mode
sandbox-mode-xml = XML append
sandbox-mode-lua = Lua append
sandbox-patch = Patch
sandbox-patch-on-change = Patch on change
sandbox-diagnostics-panel = Diagnostics panel

settings-button = Settings
settings-title = {settings-button}
settings-mod-dir = Mod directory
settings-dirs-are-mods = Treat directories as mods
settings-ftl-is-zip = Treat .ftl files as zipped mods
settings-zips-are-mods = Treat zips as mods
settings-ftl-dir = FTL data directory
settings-theme = Theme
settings-background-opacity = Background opacity

settings-advanced-header = Advanced settings
settings-disable-hs-installer = Disable Hyperspace installer
settings-autoupdate = Automatically check for updates on startup
settings-warn-missing-hs = Warn about unsatisfied Hyperspace requirements
settings-repack-archive = Repack FTL data archive
settings-repack-archive-tooltip = 
    Turning this off will slightly speed up patching but
    make the archive larger and potentially slow down startup.
    The impact mostly depends on the number of applied mods.

update-modal =
    A newer version of the mod manager is available!
    Latest version is [s]{$latest}[/s] while this version is [s]{$current}[/s].
update-modal-dismiss = Dismiss
update-modal-open-in-browser = Open in browser
update-modal-run-update = Update
update-modal-progress = Downloading update... {$current}/{$max}
update-modal-updater-unsupported =
    This build does not support automatic updates.
    Only portable builds downloaded from the releases tab can be automatically updated.

missing-hyperspace-top =
    Mod [s]{$mod}[/s] {$req ->
        [none] modifies hyperspace.xml but no Hyperspace version is selected.
       *[other] requires Hyperspace [s]{$req}[/s] but
    } {$ver ->
        [none] no Hyperspace version is currently selected.
       *[other] Hyperspace [s]{$ver}[/s] is currently selected.
    }
missing-hyperspace-middle =
    Make sure you have the correct Hyperspace version selected in the top-left
    corner of the mods list.
missing-hyperspace-bottom =
    If you are certain the enabled mods work with the selected Hyperspace version
    you can press the [i]Patch anyway[/i] button below or
    turn off this warning in the [i]Advanced settings[/i] section of
    the [i]Settings[/i] menu.
missing-hyperspace-patch-anyway = Patch anyway
