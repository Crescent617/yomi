# Tauri Linux "Origin header is not a valid URL" 500

**症状**：NixOS 上 GUI 启动时报 `Unhandled Promise Rejection: Origin header is not a valid URL`，
`plugin:notification|is_permission_granted` 返回 500；macOS 正常。

**根因**（2026-08-30 实测定位，两层）：
1. **主因**：crane 迁移（1092939a）把 flake 从 `cargo-tauri.hook` 换成裸 `cargo build -p yomi-gui`，丢了
   `--features custom-protocol`（该 feature 从不进 default，是 tauri 官方约定，否则 `tauri dev` 会坏）。
   没有它 `tauri::is_dev()` 为 true → 二进制去连 devUrl `http://localhost:1420` →
   整页 "Could not connect to localhost: Connection refused"。**修法**：flake `cargoExtraArgs` 加
   `--features custom-protocol`。
2. **次因（console 500 噪音）**：`tauri-plugin-notification` 的 `init-iife.js` 在注入时**立即**发起
   IPC（无 `.catch`）。页面没加载时 webview 停在 `about:blank`（opaque origin），请求带
   `Origin: null` → tauri `protocol.rs` `Url::parse` 失败 → 500。custom-protocol 修好后页面正常加载，
   此错仍可能在启动竞态下偶发（无害），治本等上游：
   https://github.com/tauri-apps/plugins-workspace/issues/3562

**关联**：tauri-apps/tauri#11504（同报错文案，data: URL 场景）、anomalyco/opencode#8962（同样误判为 nix/webkit 版本问题）。
调试工具链：/tmp/webkit-origin-test/（pygobject 最小复现、env.nix 按 nixpkgs rev 起 webkit 环境）。
**注意**：WebKitGTK remote inspector（WEBKIT_INSPECTOR_SERVER）不是 CDP HTTP 协议，curl /json 不通。
