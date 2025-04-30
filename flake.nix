{
  description = "An FTL: Faster Than Light mod manager";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, fenix, flake-utils, nixpkgs }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        inherit (fenix.packages.${system}.latest) toolchain;
        pkgs = nixpkgs.legacyPackages.${system};
        libs = with pkgs; with pkgs.xorg;  [
          libGL
          libGLU
          libxcb
          libXcursor
          libXrandr
          libXi
          libxkbcommon
          gtk3
          atk
          bzip2
          fontconfig
          openssl
          wayland
        ];
        libraryPath = "/run/opengl-driver/lib:${pkgs.lib.makeLibraryPath libs}";
      in
      {
        packages =
          rec {
            unwrapped = (pkgs.makeRustPlatform {
              cargo = toolchain;
              rustc = toolchain;
            }).buildRustPackage {
              pname = "ftlman-unwrapped";
              version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;

              nativeBuildInputs = with pkgs; [
                pkg-config
              ];

              buildInputs = libs ++ (with pkgs; [
                wayland
                xorg.libX11
              ]);

              src = ./.;

              cargoLock = {
                lockFile = ./Cargo.lock;
                outputHashes = {
                  "ecolor-0.31.1" = "sha256-nAt9SJCpAFpDI3cbVDohME4YSXpqcrOr3KAw5wuM1AI=";
                };
              };
            };
            # the extra parens prevent the formatter from putting the attrset on a new line
            default = (pkgs.runCommandNoCC "ftlman" {
              pname = "ftlman";
              inherit (unwrapped) version;

              nativeBuildInputs = [ pkgs.makeWrapper ];
            }) ''
              makeWrapper ${unwrapped}/bin/ftlman $out/bin/ftlman --suffix LD_LIBRARY_PATH : ${libraryPath}
            '';
          };
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
            # For llvm-strip as rust-objcopy seems to fail in apple cross containers
            llvmPackages_latest.bintools
            # for compare_with_slipstream
            difftastic
          ];
          buildInputs = libs;

          shellHook = ''
            export LD_LIBRARY_PATH=${libraryPath}
          '';
        };
      });
}
