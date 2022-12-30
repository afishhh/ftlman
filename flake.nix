{
  description = "A basic flake";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    with flake-utils.lib;
    eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;
      in
      with pkgs.lib; {
        devShell = pkgs.mkShell {
          nativeBuildInputs = with pkgs; with pkgs.xorg; [
            bashInteractive
            pkg-config
          ];
          buildInputs = with pkgs; with pkgs.xorg; [
            libX11
            libXcursor
            libXrandr
            libXi
            fontconfig
            libGL
          ];

          shellHook = ''
            export LD_LIBRARY_PATH=/run/opengl-driver/lib/:${lib.makeLibraryPath (with pkgs; [libGL libGLU])}
          '';
        };
      });
}
