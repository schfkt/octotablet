{
  description = "octotablet development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
    in
    {
      devShells = nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          nativeLibraries = with pkgs; [
            SDL2
            wayland
            libxkbcommon
          ];
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              pkg-config
            ];

            buildInputs = nativeLibraries;

            # wayland-sys loads libwayland-client dynamically, including in tests.
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath nativeLibraries;
          };
        });
    };
}
