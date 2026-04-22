{
  description = "wdotool — xdotool-compatible input automation for Wayland";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    # Linux only — wdotool depends on libxkbcommon, libwayland-client, and
    # /dev/uinput. Nothing about the project is meaningful on Darwin.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
      in
      {
        packages = rec {
          default = wdotool;
          wdotool = pkgs.rustPlatform.buildRustPackage {
            pname = manifest.name;
            version = manifest.version;
            src = pkgs.lib.cleanSource ./.;
            # Re-uses the tracked Cargo.lock for a reproducible build. All
            # deps are on crates.io, so this is all that's needed.
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = with pkgs; [ pkg-config ];
            buildInputs = with pkgs; [
              libxkbcommon
              wayland
            ];

            # Keep check() enabled — the test matrix is pure (no Wayland).
            doCheck = true;

            meta = with pkgs.lib; {
              description = manifest.description;
              homepage = "https://github.com/cushycush/wdotool";
              license = with licenses; [ mit asl20 ];
              mainProgram = "wdotool";
              platforms = platforms.linux;
            };
          };
        };

        # `nix develop` gives you the same build env plus dev-ergonomics tools.
        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.default ];
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
          ];
        };

        # `nix fmt` uses nixpkgs-fmt on .nix files.
        formatter = pkgs.nixpkgs-fmt;
      });
}
