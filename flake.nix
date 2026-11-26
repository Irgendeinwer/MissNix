{
  description = "MissNix — Desktop local music release tracker (Rust + Slint + SQLite + Deezer/MusicBrainz)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

      mkMissnix =
        pkgs:
        let
          inherit (pkgs) lib stdenv;

          runtimeGuiLibs =
            [
              pkgs.expat
              pkgs.fontconfig
              pkgs.freetype
              pkgs.zlib
            ]
            ++ lib.optionals stdenv.hostPlatform.isLinux [
              pkgs.libGL
              pkgs.vulkan-loader
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.libx11
              pkgs.libxcb
              pkgs.libxcursor
              pkgs.libxi
              pkgs.libxrandr
              pkgs.libxxf86vm
              pkgs.libpulseaudio
              pkgs.alsa-lib
            ];

          desktopItem = pkgs.makeDesktopItem {
            name = "missnix";
            desktopName = "MissNix";
            genericName = "Music Release Tracker";
            comment = "Track new releases from artists in your local music library";
            exec = "missnix";
            icon = "missnix";
            terminal = false;
            categories = [
              "AudioVideo"
              "Audio"
              "Music"
            ];
            keywords = [
              "music"
              "releases"
              "albums"
              "deezer"
              "musicbrainz"
            ];
          };
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "missnix";
          version = "0.1.0";

          src = lib.cleanSource self;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs =
            [
              pkgs.cmake
              pkgs.pkg-config
              pkgs.makeWrapper
            ]
            ++ lib.optionals stdenv.hostPlatform.isLinux [
              pkgs.copyDesktopItems
            ];

          buildInputs =
            runtimeGuiLibs
            ++ [
              pkgs.sqlite
            ]
            ++ lib.optionals stdenv.hostPlatform.isLinux [
              pkgs.libxcb.dev
            ];

          desktopItems = lib.optionals stdenv.hostPlatform.isLinux [
            desktopItem
          ];

          cargoTestFlags = [
            "--package"
            "missnix-core"
          ];

          postInstall = ''
            install -Dm644 assets/missnix.svg \
              $out/share/icons/hicolor/scalable/apps/missnix.svg
          ''
          + lib.optionalString stdenv.hostPlatform.isLinux ''
            wrapProgram $out/bin/missnix \
              --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath runtimeGuiLibs}" \
              --suffix PATH : "${
                lib.makeBinPath [
                  pkgs.xdg-utils
                  pkgs.zenity
                ]
              }"
          '';

          meta = {
            description = "Standalone desktop release radar for artists in your local music collection";
            license = lib.licenses.mit;
            mainProgram = "missnix";
            platforms = supportedSystems;
          };
        };
    in
    {
      overlays.default = final: _prev: {
        missnix = mkMissnix final;
      };

      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          missnix = mkMissnix pkgs;
        in
        {
          inherit missnix;
          default = missnix;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/missnix";
          meta = {
            description = "MissNix local music release tracker";
          };
        };
      });

      checks = forAllSystems (system: {
        missnix = self.packages.${system}.default;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs) lib stdenv;
          guiLibs =
            [
              pkgs.fontconfig
              pkgs.freetype
            ]
            ++ lib.optionals stdenv.hostPlatform.isLinux [
              pkgs.libGL
              pkgs.vulkan-loader
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.libx11
              pkgs.libxcb
              pkgs.libxcursor
              pkgs.libxi
              pkgs.libxrandr
              pkgs.libpulseaudio
              pkgs.alsa-lib
            ];
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.default ];

            packages = [
              pkgs.rustc
              pkgs.cargo
              pkgs.clippy
              pkgs.rustfmt
              pkgs.rust-analyzer
              pkgs.slint-lsp
              pkgs.sqlite
            ]
            ++ lib.optionals stdenv.hostPlatform.isLinux [
              pkgs.zenity
              pkgs.xdg-utils
            ];

            LD_LIBRARY_PATH = lib.optionalString stdenv.hostPlatform.isLinux (lib.makeLibraryPath guiLibs);
          };
        }
      );
    };
}
