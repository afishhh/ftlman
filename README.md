## Another [FTL: Faster Than Light](https://subsetgames.com/ftl.html) Mod Manager

This project is an alternative to the [Slipstream Mod Manager](https://github.com/Vhati/Slipstream-Mod-Manager), written in Rust and with some additional features.

### Features

- [x] Regular FTL mods (Slipstream compatible)
- [x] Automatic [Hyperspace](https://github.com/FTL-Hyperspace/FTL-Hyperspace) installer

> [!WARNING]
> There may be bugs in my implementation of Slipstream's .xml.append format. If you encounter issues please verify you are using the latest version of ftlman and make sure the issue is a bug in ftlman and not the mod itself, then [open an issue](https://github.com/afishhh/ftlman/issues/new) on GitHub. Make sure to provide all the information necessary to reproduce the bug.

Currently automatic hyperspace installation is only supported on the following OS + Store combinations:
- Windows + Steam
- Linux + Steam

Adding support for other game stores is possible, but I only own the game on Steam so I have no way of testing potential implementations. If anyone wants an implementation and is willing to test changes for me, please reach out (opening an issue works).

### Installation

#### Pre-built binaries

Pre-built binaries for both Linux and Windows are available in the [Releases](https://github.com/afishhh/ftlman/releases) tab.

#### Compiling from source

Compilation requires a **nightly** Rust toolchain due to questionable design decisions I made in [silpkg](https://github.com/afishhh/silpkg).
For instructions on installing Rust go to https://www.rust-lang.org/tools/install, make sure to select the nightly toolchain release during installation.

After installing Rust, open a terminal then and execute the following command:
```command
cargo install --git https://github.com/afishhh/ftlman
```

> [!NOTE]
> The same command can be used to update the program.

You should then be able to run `ftlman` in a terminal to run the program.

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

You can also temporarily try out the program using `nix run`:

```command
nix run github:afishhh/ftlman
```

### Contributing

I highly discourage anyone from opening or working on pull requests right now since I am not satisfied with the code quality of this project and may make significant changes at any moment.
