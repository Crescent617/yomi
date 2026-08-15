{
  description = "Yomi - AI coding assistant with TUI, CLI, and Tauri GUI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

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

        # 前端 npm 依赖 hash（基于 crates/gui/frontend/package-lock.json 计算）
        npmDepsHash = pkgs.lib.fakeHash;

        commonNativeBuildInputs = with pkgs; [
          pkg-config
          cmake
          protobuf
        ];

        commonBuildInputs = with pkgs; [
          openssl
        ];

        commonMeta = with pkgs.lib; {
          license = licenses.mit;
          homepage = "https://github.com/Crescent617/yomi";
        };

        rustDev = with pkgs; [
          rustc
          cargo
          clippy
          rustfmt
          rust-analyzer
        ];

      in {
        apps = {
          yomi-cli = flake-utils.lib.mkApp { drv = self.packages.${system}.yomi-cli; };
          yomi-gui = flake-utils.lib.mkApp { drv = self.packages.${system}.yomi-gui; };
          default = flake-utils.lib.mkApp { drv = self.packages.${system}.yomi-cli; };
        };

        packages = {
          yomi-cli = pkgs.rustPlatform.buildRustPackage {
            inherit src version;
            pname = "yomi-cli";

            cargoLock = {
              lockFile = ./Cargo.lock;
              allowBuiltinFetchGit = true;
            };

            cargoBuildFlags = [ "-p" "cli" ];

            nativeBuildInputs = commonNativeBuildInputs;
            buildInputs = commonBuildInputs;

            # 构建阶段的 checkPhase 在 sandbox 中测试 kernel 包需要 ripgrep/git，
            # 而 cargoCheckFlags 在 nixpkgs 的 cargoCheckHook 中实际不生效（被当作 test binary 参数）。
            # 测试应在开发环境或 CI 中单独运行：cargo test -p cli
            doCheck = false;

            meta = commonMeta // {
              description = "Yomi CLI - AI coding assistant command-line interface";
              mainProgram = "yomi";
            };
          };

          yomi-gui = pkgs.rustPlatform.buildRustPackage {
            inherit src version;
            pname = "yomi-gui";

            cargoLock = {
              lockFile = ./Cargo.lock;
              allowBuiltinFetchGit = true;
            };

            # Tauri 后端在 workspace 子目录中。
            # 注意：workspace 项目的 Cargo.lock 在根目录，因此不设置 cargoRoot
            #（cargoRoot 控制 cargoDeps 的 lockfile 查找路径）。
            # buildAndTestSubdir 仅用于构建/测试阶段的 pushd，不影响 cargoDeps 生成。
            buildAndTestSubdir = "crates/gui";

            # 前端 npm 依赖（package-lock.json 在 crates/gui/frontend/）
            npmDeps = pkgs.fetchNpmDeps {
              src = ./crates/gui/frontend;
              hash = npmDepsHash;
            };

            nativeBuildInputs = with pkgs; [
              cargo-tauri.hook
              nodejs
              npmHooks.npmConfigHook
              pkg-config
              wrapGAppsHook4
              makeWrapper
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

            # npmHooks.npmConfigHook 在 crates/gui/frontend/ 运行 npm ci
            npmRoot = "crates/gui/frontend";

            # 禁用 beforeBuildCommand，避免递归（preBuild 已手动构建前端）
            postPatch = ''
              substituteInPlace crates/gui/tauri.conf.json \
                --replace-fail '"beforeBuildCommand": "cd frontend && npm run build"' \
                '"beforeBuildCommand": "true"'
            '';

            # 在 cargo tauri build 之前手动构建前端
            # cargo-tauri.hook 的 beforeBuildCommand 在递归调用中可能被跳过
            preBuild = ''
              pushd crates/gui/frontend
              npm run build
              popd
            '';

            # Tauri 构建在 sandbox 中无法运行测试（需要显示/WebKit），且 workspace 测试依赖外部命令
            doCheck = false;

            postFixup = ''
              # 确保前端产物在 store 中的正确位置
              mkdir -p $out/frontend
              cp -r crates/gui/frontend/build $out/frontend/

              # wrapGAppsHook 在 fixupPhase 的 main body 中已经创建了 wrapper
              # 我们在其 wrapper 之上再包一层 cd wrapper
              if [ -f "$out/bin/.yomi-gui-wrapped" ]; then
                mv $out/bin/yomi-gui $out/bin/.yomi-gui-gtk-wrapped
                makeWrapper $out/bin/.yomi-gui-gtk-wrapped $out/bin/yomi-gui \
                  --chdir "$out" \
                  --inherit-argv0
              fi
            '';

            meta = commonMeta // {
              description = "Yomi GUI - Tauri-based AI coding assistant desktop app";
              mainProgram = "yomi-gui";
            };
          };

          default = self.packages.${system}.yomi-cli;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          # yomi 的 docker 镜像：yomi + 基础工具链，无配置无密钥。
          # nix build .#dockerImage && docker load < result  →  yomi:<version>
          # （本地/复现路径；CI 发版镜像走 docker/Dockerfile + 预编译二进制，
          #   两边工具集保持一致，改动时同步）
          dockerImage =
            let
              yomi = self.packages.${system}.yomi-cli;
            in
            pkgs.dockerTools.buildLayeredImage {
              name = "yomi";
              tag = version;

              # 基础工具链 + 常规 unix 用户态；node 等可选工具链由 agent 运行时自装
              contents = with pkgs; [
                yomi
                bashInteractive
                coreutils
                curl
                diffutils
                findutils
                gawk
                git
                gnugrep
                gnused
                gnutar
                gzip
                jq
                ncurses # tmux 需要 terminfo
                openssh
                procps
                python3
                ripgrep
                tmux
                which
                xz
                # FHS 兼容件：/bin/sh、/usr/bin/env、CA bundle、/etc/passwd
                dockerTools.binSh
                dockerTools.usrBinEnv
                dockerTools.caCertificates
                dockerTools.fakeNss
              ];

              extraCommands = ''
                mkdir -p home/yomi
                install -d -m 1777 tmp
              '';

              config = {
                Entrypoint = [ "${yomi}/bin/yomi" "daemon" "start" ];
                WorkingDir = "/home/yomi";
                Env = [
                  "HOME=/home/yomi"
                  "PATH=/bin:/usr/bin"
                  "LANG=C.UTF-8"
                  "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                  "NIX_SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                  "GIT_SSL_CAINFO=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                  "TERMINFO_DIRS=${pkgs.ncurses}/share/terminfo"
                ];
              };
            };
        };

        devShells = {
          default = pkgs.mkShell {
            nativeBuildInputs = commonNativeBuildInputs ++ rustDev ++ [ pkgs.nodejs ];
            buildInputs = commonBuildInputs;
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
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
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        };
      }
    );
}
