{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = with pkgs; [
    # Rust toolchain (assumes rustup is used; remove if you prefer nixpkgs rustc)
    rustup
    cacert

    # Tauri v2 Linux dependencies
    pkg-config
    dbus
    glib
    gtk3
    libsoup_3
    webkitgtk_4_1
    openssl
    curl
    libappindicator

    # Common C/C++ build deps
    cmake
    gcc
    gnumake

    # Node.js for frontend (includes npm)
    nodejs_22

    # Additional libraries often required
    cairo
    pango
    gdk-pixbuf
    harfbuzz
    at-spi2-atk
    libdrm
    mesa
    alsa-lib
    systemd # for libudev
  ];

  shellHook = ''
    echo "Yomi GUI dev shell ready"
    echo "  cargo  : $(cargo --version)"
    echo "  node   : $(node --version)"
    export PATH="$PWD/crates/gui/frontend/node_modules/.bin:$PATH"
  '';
}
