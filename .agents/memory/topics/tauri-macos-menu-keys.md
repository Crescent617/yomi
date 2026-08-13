# Tauri macOS 默认菜单吞按键

**根因**:Tauri v2 默认 `enable_macos_default_menu: true`,`Menu::default()` 的 Edit 子菜单含预置 Undo (Cmd+Z) / Redo (Cmd+Shift+Z) / Cut / Copy / Paste / Select All。macOS 的菜单 key equivalent 由 OS 在**事件送达 webview 之前**匹配——匹配到就走原生 `undo:` 响应链,页面连 keydown 都收不到。

**影响**:对 CodeMirror/Monaco 这类 JS 自管历史的编辑器,原生 undo 完全无效(表现为"编辑器里 Cmd+Z 没反应");对原生 textarea 反而有效(所以手搓 textarea 时代没人发现)。Cmd+Backspace 等其他编辑键也有类似的 WKWebView 原生行为。

**修复**(v0.7.76,`crates/gui/src/main.rs` 的 `app_menu`):自建菜单,逐项复制默认菜单但 Edit 摘掉 Undo/Redo,Cut/Copy/Paste/Select All 保留原生(对 CM 和 textarea 都正常)。摘掉后 Cmd+Z 回落到页面:CM 由 keymap 处理,textarea 走 WebKit 内建撤销。注意:About 项要手动传 `AboutMetadata`(name/version/copyright),否则 About 面板丢版本信息;Window 子菜单保留 `WINDOW_SUBMENU_ID`。

**验证手段**:Playwright webkit 引擎 ≈ WKWebView,可直接验证编辑器按键行为(`page.keyboard.press("Meta+z")`);但菜单拦截只在打出来的 macOS app 里存在,browser 测不出来。
