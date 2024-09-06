## Another [FTL: Faster Than Light](https://subsetgames.com/ftl.html) Mod Manager

This project is an alternative to the [Slipstream Mod Manager](https://github.com/Vhati/Slipstream-Mod-Manager), written in Rust and with some additional features.

### Features

- [x] Regular FTL mods (mostly Slipstream compatible*)
- [x] Automatic [Hyperspace](https://github.com/FTL-Hyperspace/FTL-Hyperspace) installer

Currently automatic hyperspace installation is only supported on the following OS + Store combinations:
- Windows + Steam
- Linux + Steam

Adding support for other game stores is possible, but I only own the game on Steam so I have no way of testing potential implementations. If anyone wants an implementation and is willing to test changes for me, [open an issue](https://github.com/afishhh/ftlman/issues/new).

\* Most Slipstream mods are supported but these Slipstream features are currently unimplemented:
- .rawappend.xml/.xml.rawappend files
- .rawclobber.xml/.xml.rawclobber files
- Some ill-formed XML files may still be rejected by ftlman but accepted by Slipstream.

These features are not particularly difficult to implement but I just don't have any mods that actually *use* these features so I can't test them. If you find a mod that requires this please [open an issue](https://github.com/afishhh/ftlman/issues/new).

If you a mod that fails to patch with ftlman but works with slipstream or one that works different under ftlman also [open an issue](https://github.com/afishhh/ftlman/issues/new).

### Installation

#### Pre-built binaries

Pre-built binaries for both Linux and Windows are available in the [Releases](https://github.com/afishhh/ftlman/releases) tab.

#### Compiling from source

Compilation requires a **nightly** Rust toolchain due to questionable design decisions I made in [silpkg](https://github.com/afishhh/silpkg).
For instructions on installing Rust go to https://www.rust-lang.org/tools/install, make sure to select the nightly toolchain release during installation.

After installing Rust, open a terminal then execute the following command:
```command
cargo install --git https://github.com/afishhh/ftlman
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
