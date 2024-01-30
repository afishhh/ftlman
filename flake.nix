{
  description = "A basic flake";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, fenix, flake-utils, nixpkgs }:
    flake-utils.lib.eachDefaultSystem (system: {
      packages =
        let
          inherit (fenix.packages.${system}.latest) toolchain;
          pkgs = nixpkgs.legacyPackages.${system};
          runtimeLibs = "/run/opengl-driver/lib/:${pkgs.lib.makeLibraryPath (with pkgs; [ libGL libGLU libxkbcommon ])}";

        in
        rec {
          unwrapped = (pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          }).buildRustPackage {
            pname = "ftlman-unwrapped";
            version = "0.1.0";

            nativeBuildInputs = with pkgs; [
              pkg-config
            ];

            buildInputs = with pkgs; with pkgs.xorg; [
              libX11
              libXcursor
              libXrandr
              libXi
              fontconfig
              openssl
            ];

            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
              outputHashes = {
                # silpkg's internal macros crate
                # buildRustPackage doesn't seem to deal with such subpackages seamlessly
                "macros-0.0.0" = "sha256-3z553VbZRNpO8vtmZsBbqsW3IOLdrYcurHtX+Dml2I0=";
              };
            };

            shellHook = ''
              export LD_LIBRARY_PATH=${runtimeLibs}
            '';
          };
          default = pkgs.runCommandNoCC "ftlman"
            {
              pname = "ftlman";
              inherit (unwrapped) version;

              nativeBuildInputs = [ pkgs.makeWrapper ];
            } ''
            makeWrapper ${unwrapped}/bin/ftlman $out/bin/ftlman --suffix LD_LIBRARY_PATH : ${runtimeLibs}
          '';
        };
        devShells.default = self.packages.${system}.unwrapped;
    });
}
