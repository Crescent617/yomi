{
  description = "Yomi - AI coding assistant with TUI, CLI, and Tauri GUI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # crane：把依赖编译拆成独立 derivation，改源码不再全量重编依赖
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;
        craneLib = crane.mkLib pkgs;

        # 从 Cargo.toml 自动读取 workspace 版本
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

        # 只保留构建必需：根 Cargo.toml/Cargo.lock + crates/ 下的源码。
        # docs/site/evals/examples/scripts 等改动不再触发重编；
        # 构建产物与缓存目录（target/node_modules/前端 build 产物等）一律排除。
        src = lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            let
              rel = lib.removePrefix (toString ./. + "/") (toString path);
              base = baseNameOf path;
              junkDirs = [
                "target"
                "node_modules"
                "build"
                "dist"
                ".svelte-kit"
                ".vite"
                "test-results"
              ];
            in
            (rel == "Cargo.toml" || rel == "Cargo.lock" || rel == "crates" || lib.hasPrefix "crates/" rel)
            && !(type == "directory" && builtins.elem base junkDirs)
            && !(lib.hasSuffix ".log" base);
        };

        commonNativeBuildInputs = with pkgs; [
          pkg-config
          cmake
          protobuf
        ];

        commonBuildInputs = with pkgs; [
          openssl
        ];

        # Tauri/WebKit 系统依赖（gui 的 sys crate 编译需要，仅 Linux）
        guiBuildInputs = with pkgs; lib.optionals stdenv.isLinux [
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
        ];

        commonMeta = {
          license = lib.licenses.agpl3Only;
          homepage = "https://github.com/Crescent617/yomi";
        };

        # crane 共享构建参数
        commonArgs = {
          inherit src;
          strictDeps = true;
          # sandbox 中测试需要 ripgrep/git/显示服务；测试在 dev shell 或 CI 中单独跑：cargo test
          doCheck = false;
          nativeBuildInputs = commonNativeBuildInputs;
          buildInputs = commonBuildInputs ++ guiBuildInputs;
        };

        # 依赖预编译：workspace 全部第三方依赖单独一个 derivation，cli/gui 共享。
        # 只有 Cargo.toml/Cargo.lock 变化时才重编依赖（crane 内部用 dummy 源码，
        # 源码改动不影响本 derivation）；日常改代码只重编 workspace 自身 crate。
        # 注意：因为是整 workspace 的依赖，首次构建会编译包括 tauri/webkit 在内的
        # 全部依赖（一次性成本），之后 cli 与 gui 的构建都复用这份缓存。
        cargoArtifacts = craneLib.buildDepsOnly (commonArgs // {
          pname = "yomi-deps";
          inherit version;
        });

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
          yomi-cli = craneLib.buildPackage (commonArgs // {
            inherit version cargoArtifacts;
            pname = "yomi-cli";

            cargoExtraArgs = "-p cli";

            meta = commonMeta // {
              description = "Yomi CLI - AI coding assistant command-line interface";
              mainProgram = "yomi";
            };
          });

          yomi-gui = craneLib.buildPackage (commonArgs // {
            inherit version cargoArtifacts;
            pname = "yomi-gui";

            # custom-protocol：tauri 用它区分生产/开发模式（见 tauri 官方模板约定，勿放进 default
            # feature，否则 tauri dev 的 devUrl/HMR 会失效）。缺失时二进制误以为 dev 去连
            # devUrl localhost:1420 → 整页 "Could not connect to localhost: Connection refused"。
            cargoExtraArgs = "-p yomi-gui --features custom-protocol";

            # 前端 npm 依赖（直接读取 crates/gui/frontend/package-lock.json，无需维护 hash）
            npmDeps = pkgs.importNpmLock { npmRoot = ./crates/gui/frontend; };
            npmRoot = "crates/gui/frontend";

            nativeBuildInputs = commonArgs.nativeBuildInputs ++ (with pkgs; [
              nodejs
              importNpmLock.npmConfigHook
              makeWrapper
            ]) ++ lib.optionals pkgs.stdenv.isLinux [
              pkgs.wrapGAppsHook4
            ];

            # 在 cargo build 前手动构建前端：tauri-build 会把 frontendDist 嵌进二进制
            preBuild = ''
              pushd crates/gui/frontend
              npm run build
              popd
            '';

            # 复用同一 target dir 顺带编译 CLI：deps 已编译过，只需编译 cli crate 本身
            postBuild = ''
              cargo build --release -p cli
            '';

            postFixup = ''
              # applications 菜单条目（a47b19a 重构时丢失，在此恢复）
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

              # 随 app 一并分发 CLI（postBuild 已编译），放在 GTK wrap 之后避免被 wrapGAppsHook 包装
              install -Dm755 target/release/yomi $out/bin/yomi
            '';

            meta = commonMeta // {
              description = "Yomi GUI - Tauri-based AI coding assistant desktop app";
              mainProgram = "yomi-gui";
            };
          });

          default = self.packages.${system}.yomi-cli;
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
            ] ++ lib.optionals pkgs.stdenv.isLinux [
              pkgs.wrapGAppsHook4
            ];
            buildInputs = commonBuildInputs ++ guiBuildInputs;
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        };
      }
    );
}
