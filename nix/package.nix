# Rockhopper, for `pkgs.callPackage`. Needs Rust 1.92 or newer (nixos-26.05
# or unstable).
{
  lib,
  rustPlatform,
  makeWrapper,
  pkg-config,
  wrapGAppsHook3,
  fontconfig,
  freetype,
  libxcb,
  libxkbcommon,
  vulkan-loader,
  vulkan-headers,
  wayland,
  webkitgtk_4_1,
  gtk3,
  libsoup_3,
  glib-networking,
}:
let
  cargo = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargo.package.name;
  inherit (cargo.package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    makeWrapper
    pkg-config
    wrapGAppsHook3
  ];
  buildInputs = [
    fontconfig
    freetype
    libxcb
    libxkbcommon
    vulkan-loader
    vulkan-headers
    wayland
    # The window for MCP Apps.
    webkitgtk_4_1
    gtk3
    libsoup_3
    glib-networking
  ];

  # GPUI dlopens Vulkan, Wayland and xkbcommon at runtime.
  postFixup = ''
    wrapProgram $out/bin/rockhopper \
      --prefix LD_LIBRARY_PATH : ${
        lib.makeLibraryPath [
          libxkbcommon
          vulkan-loader
          wayland
        ]
      }
  '';

  meta = {
    description = "A native GPUI launcher for Wayland, much like Raycast";
    homepage = "https://github.com/korora-tech/rockhopper";
    license = lib.licenses.asl20;
    mainProgram = "rockhopper";
    platforms = lib.platforms.linux;
  };
}
