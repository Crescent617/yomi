# Tauri Linux "Origin header is not a valid URL" 500

**症状**：NixOS 上 GUI 启动时报 `Unhandled Promise Rejection: Origin header is not a valid URL`，
`plugin:notification|is_permission_granted` 返回 500；macOS 正常。

**根因**（2026-08-30 实测定位）：`tauri-plugin-notification` 的 `init-iife.js` 在注入时**立即**发起
IPC（不等页面加载、无 `.catch`）。WebKitGTK 下 document-start user script 可能跑在 webview 初始
`about:blank` 文档上（竞态，桌面环境越重越容易触发；Xvfb 下反而复现不了），此时请求带
`Origin: null` + `Referer: about:blank`（opaque origin），tauri `protocol.rs` 对 Origin 做
`Url::parse` 失败 → 500。**与 webkitgtk 版本无关**（2.52.4 实测正常页面 Origin 是对的）。

**验证方法**：patch tauri `ipc/protocol.rs`，Origin parse 失败时 eprintln 原始头（一眼看到
`"null"` + `about:blank`）。

**修复**：vendor 插件给 init script 加 `"about:blank"!==location.href &&` 前缀 + `.catch(()=>{})`。
上游 issue: https://github.com/tauri-apps/plugins-workspace/issues/3562

**关联**：tauri-apps/tauri#11504（同报错文案，data: URL 场景）、anomalyco/opencode#8962（同样误判为 nix/webkit 版本问题）。
调试工具链：/tmp/webkit-origin-test/（pygobject 最小复现、env.nix 按 nixpkgs rev 起 webkit 环境）。
**注意**：WebKitGTK remote inspector（WEBKIT_INSPECTOR_SERVER）不是 CDP HTTP 协议，curl /json 不通。
