# nixpkgs rustc 1.95.0 / LLVM 21 ThinLTO 段错误

- 现象：nix 构建 yomi-gui 时 rustc 编译 `notify-rust` SIGSEGV，backtrace 停在
  `llvm::ValueEnumerator::EnumerateType` → `ThinLTOBitcodeWriterPass::run`；
  同次构建 binutils 2.46 的 ld 也偶发 segfault。
- 排除：内存充足无 OOM；内核日志显示崩溃地址与栈指针差 ~2GB，是野指针读而非栈溢出
  → `RUST_MIN_STACK` 无效（且 shell env 本来就进不了 nix 沙箱）。
- 规避：`-C codegen-units=1`（关掉 thin local LTO，完全绕过崩溃的 pass）。
  2026-08-22 打在 `/etc/nixos/nyx/flake.nix` 的 yomi-app overrideAttrs（RUSTFLAGS + RUST_MIN_STACK），
  已验证构建通过。nixpkgs 工具链修复后可移除。
- 教训：nix 沙箱构建的修复必须打进 drv（overrideAttrs），shell 环境变量无效。
