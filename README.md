## Another [FTL: Faster Than Light](https://subsetgames.com/ftl.html) Mod Manager

This project is an alternative to the [Slipstream Mod Manager](https://github.com/Vhati/Slipstream-Mod-Manager), written in Rust and with some additional features.

### Features

- [x] Regular FTL mods (Slipstream compatible¹)
- [x] Supports mod tags from [Blizz's Slipstream fork](https://github.com/blizzarchon/Slipstream-Mod-Manager)
- [x] Supports an ftlman-specific Lua patching API (documentation available [here](https://fishhh.dev/ftlman/lua.html))
- [x] Automatic [Hyperspace](https://github.com/FTL-Hyperspace/FTL-Hyperspace) installer
- [x] Functional² XML and Lua Sandbox (syntax highlighting, WIP diagnostics)

Currently automatic hyperspace installation is supported for the following FTL versions:
- Steam (Linux and Windows)
- Epic
- Origin (untested)

Patching the Microsoft version is technically supported but you may encounter permission problems if you try to patch it in the original installation directory.

If you find a mod that fails to patch with ftlman but works with slipstream or one that works different under ftlman [open an issue](https://github.com/afishhh/ftlman/issues/new).

¹ ftlman's XML parser is modeled closely after FTL's (RapidXML). This means it is very lenient but behaviour may be slightly different than Slipstream, due to Slipstream's messy handling of sloppy parsing.<br>
² Struggles with files hundreds of kilobytes in size.

### Installation

#### Pre-built binaries

Pre-built binaries for Linux, MacOS and Windows are available in the [Releases](https://github.com/afishhh/ftlman/releases) tab.

#### Compiling from source

Compilation requires a **nightly** Rust toolchain due to questionable design decisions I made in [silpkg](https://github.com/afishhh/silpkg).
For instructions on installing Rust go to https://www.rust-lang.org/tools/install, make sure to select the nightly toolchain release during installation.

After installing Rust, open a terminal then execute the following command:
```command
cargo install --locked --git https://github.com/afishhh/ftlman ftlman
```

> [!NOTE]
> The same command can be used to update the program.

You should then be able to run `ftlman` in a terminal to start the program.

> [!NOTE]
> These instructions also apply to Windows users, just replace terminal with `cmd.exe`.

#### NixOS with flakes

Add this repository to your flake inputs and then use the default package output.

```nix
{
  # ... other inputs ...
  inputs.ftlman.url = "github:afishhh/ftlman";
  # Optionally reuse top-level nixpkgs
  # inputs.ftlman.inputs.nixpkgs.follows = "nixpkgs";

  outputs = { self, ftlman, ... }:
    let
      package = ftlman.packages.${system}.default;
    in {
      # use package
    };
}
```

You can also try out the program using `nix run`:

```command
nix run github:afishhh/ftlman
```
