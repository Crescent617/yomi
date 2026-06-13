{
  description = "Yomi - AI coding assistant with TUI, CLI, and Tauri GUI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, nixpkgs-unstable, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        # Unstable has rustc 1.94+ which sqlx 0.9 and tauri-plugin-pilot require
        pkgs-unstable = nixpkgs-unstable.legacyPackages.${system};

        # 从 Cargo.toml 自动读取 workspace 版本
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

        # 严格过滤：排除所有构建产物、开发环境文件及日志
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            let
              base = baseNameOf path;
              isJunk = builtins.elem base [
                "target"
                "node_modules"
                "build"
                "dist"
                ".svelte-kit"
                ".vite"
                ".cargo"
                ".git"
                ".github"
                ".vscode"
                ".direnv"
                ".envrc"
                ".eslintcache"
                ".stylelintcache"
                ".prettiercache"
                "result"
              ]
              || pkgs.lib.hasPrefix "result-" base
              || pkgs.lib.hasSuffix ".log" base
              || pkgs.lib.hasSuffix ".tmp" base
              || pkgs.lib.hasSuffix ".tsbuildinfo" base;
            in
              !isJunk && pkgs.lib.cleanSourceFilter path type;
        };

        cargoLock = {
          lockFile = ./Cargo.lock;
          outputHashes = {
            "fix-path-env-0.0.0" = "sha256-UygkxJZoiJlsgp8PLf1zaSVsJZx1GGdQyTXqaFv3oGk=";
            "tauri-plugin-pilot-0.6.0" = "sha256-S7brFCDqpXoPBNYIQdakLxbmmlZYSrodniRe44m3Ir0=";
          };
        };

        npmDeps = pkgs.fetchNpmDeps {
          src = ./crates/gui/frontend;
          hash = "sha256-xUEzQYIs7JZsvyfJnv1QXwCDr+ezbHLvkRwbJuCdPt4=";
        };

        commonNativeBuildInputs = with pkgs; [
          pkg-config
          cmake
        ];

        commonBuildInputs = with pkgs; [
          openssl
        ];

        # Unstable rustPlatform for newer rustc (1.94+)
        rustPlatform = pkgs-unstable.rustPlatform;

        commonMeta = with pkgs.lib; {
          license = licenses.mit;
          homepage = "https://github.com/Crescent617/yomi";
        };

        yomi-cli = rustPlatform.buildRustPackage {
          inherit src cargoLock version;
          pname = "yomi-cli";

          cargoBuildFlags = [ "-p" "cli" ];
          cargoCheckFlags = [ "-p" "cli" ];

          nativeBuildInputs = commonNativeBuildInputs;
          buildInputs = commonBuildInputs;

          meta = commonMeta // {
            description = "Yomi CLI - AI coding assistant command-line interface";
            mainProgram = "yomi";
          };
        };

        yomi-gui = rustPlatform.buildRustPackage {
          inherit src cargoLock npmDeps version;
          pname = "yomi-gui";

          cargoBuildFlags = [ "-p" "yomi-gui" ];
          cargoCheckFlags = [ "-p" "yomi-gui" ];

          nativeBuildInputs = with pkgs; [
            npmHooks.npmConfigHook
            nodejs
            wrapGAppsHook4
          ] ++ commonNativeBuildInputs;

          buildInputs = with pkgs; [
            webkitgtk_4_1
            gtk3
            libsoup_3
            libappindicator-gtk3
            glib
            gdk-pixbuf
            pango
            cairo
            harfbuzz
            at-spi2-atk
            dbus
          ] ++ commonBuildInputs;

          npmRoot = "crates/gui/frontend";

          # Build frontend before cargo build so tauri-build can find frontendDist
          preBuild = ''
            cd crates/gui/frontend
            npm run build
            cd ../..
          '';

          postInstall = ''
            install -Dm644 crates/gui/icons/128x128.png $out/share/icons/hicolor/128x128/apps/yomi.png
            install -Dm644 crates/gui/icons/32x32.png $out/share/icons/hicolor/32x32/apps/yomi.png

            mkdir -p $out/share/applications
            cat > $out/share/applications/yomi.desktop <<EOF
[Desktop Entry]
Name=Yomi
Exec=yomi-gui
Icon=yomi
Type=Application
Categories=Utility;
EOF
          '';

          meta = commonMeta // {
            description = "Yomi GUI - Tauri-based AI coding assistant desktop app";
            mainProgram = "yomi-gui";
          };
        };

        rustDev = [
          pkgs-unstable.rustc
          pkgs-unstable.cargo
          pkgs-unstable.clippy
          pkgs-unstable.rustfmt
          pkgs-unstable.rust-analyzer
        ];

      in {
        apps = {
          yomi-cli = flake-utils.lib.mkApp { drv = yomi-cli; };
          yomi-gui = flake-utils.lib.mkApp { drv = yomi-gui; };
          default = flake-utils.lib.mkApp { drv = yomi-cli; };
        };

        packages = {
          inherit yomi-cli yomi-gui;
          default = yomi-cli;
        };

        devShells = {
          default = pkgs.mkShell {
            nativeBuildInputs = commonNativeBuildInputs ++ rustDev ++ [ pkgs.nodejs ];
            buildInputs = commonBuildInputs;
            RUST_SRC_PATH = "${pkgs-unstable.rustPlatform.rustLibSrc}";
          };

          gui = pkgs.mkShell {
            nativeBuildInputs = commonNativeBuildInputs ++ rustDev ++ [
              pkgs.nodejs
              pkgs.npmHooks.npmConfigHook
              pkgs.wrapGAppsHook4
            ];
            buildInputs = with pkgs; [
              webkitgtk_4_1
              gtk3
              libsoup_3
              libappindicator-gtk3
              glib
              gdk-pixbuf
              pango
              cairo
              harfbuzz
              at-spi2-atk
              dbus
            ] ++ commonBuildInputs;
            RUST_SRC_PATH = "${pkgs-unstable.rustPlatform.rustLibSrc}";
          };
        };
      }
    );
}
