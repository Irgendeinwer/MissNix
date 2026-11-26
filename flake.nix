{
  description = "A desktop release radar for your local music library — scan your folders, track favorite artists, and miss nothing";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      cargoMeta = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      pkgVersion = cargoMeta.workspace.package.version or cargoMeta.package.version;

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
            genericName = "Music Release Radar";
            comment = "Scan your local music folders, track favorite artists, and miss nothing";
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
          version = pkgVersion;

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
            description = "Desktop release radar for your local music library — scan your folders, track favorite artists, and miss nothing";
            homepage = "https://github.com/Irgendeinwer/MissNix";
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
            description = "Desktop release radar for your local music library";
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
