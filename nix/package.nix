{
  lib,
  rustPlatform,
  pkg-config,
  aws-lc,
  dbus,
  fontconfig,
  freetype,
  libGL,
  libgbm,
  libxcb,
  libxkbcommon,
  pipewire,
  sqlite,
  vulkan-loader,
  wayland,
  ffmpeg-headless,
  pulseaudio,
  onnxruntime,
  # Text recognition and the local image models (ADR 023) run on ONNX Runtime, which adds about
  # 240 MiB. Without it the app downloads the upstream build on first use, which loads only where
  # `libstdc++` is in a standard library folder (not on a stock NixOS).
  withOnnxruntime ? true,
}:

let
  workspace = (lib.importTOML ../Cargo.toml).workspace.package;

  # Recording runs FFmpeg with `x11grab` (X11), `pulse` (audio) and the encoders Laqta offers.
  # Built from the headless variant, whose codec libraries PipeWire already pulls in, it adds
  # ~35 MiB, where the default `ffmpeg` adds ~330 MiB and still lacks `x11grab`.
  ffmpeg = ffmpeg-headless.override {
    withPulse = true;
    withXcb = true;
    withXcbShm = true;
    withXcbxfixes = true; # `-draw_mouse`
  };

  # Programs Laqta starts by name: FFmpeg for recording, `pactl` to list audio devices.
  tools = [
    ffmpeg
    pulseaudio
  ];

  # Opened with dlopen at run time (the binary names them but does not link them), so they go
  # on the RUNPATH: D-Bus, EGL, Vulkan and the Wayland client.
  dlopenLibs = [
    dbus
    libGL
    vulkan-loader
    wayland
  ];
in
rustPlatform.buildRustPackage {
  pname = "laqta";
  inherit (workspace) version;

  # Only what the two binaries compile from, so docs or CI edits do not rebuild the package.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../.cargo
      ../Cargo.toml
      ../Cargo.lock
      ../apps
      ../assets/icon
      ../crates
      ../locales
      ../packaging
      ../vendor
      ../xtask
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  # The profile of the downloadable builds (Cargo.toml): longer to build, smaller to download.
  buildType = "dist";

  cargoBuildFlags = [
    "--package=laqta"
    "--package=laqta-nmh"
  ];

  nativeBuildInputs = [
    pkg-config
    rustPlatform.bindgenHook
  ];

  buildInputs = [
    aws-lc # rustls' crypto; nixpkgs links aws-lc-sys to it instead of compiling a copy
    fontconfig
    freetype
    libgbm
    libxcb
    libxkbcommon
    pipewire
    sqlite # nixpkgs links rusqlite to it (built with FTS5, which history search needs)
  ]
  ++ dlopenLibs;

  # Read with option_env! in apps/laqta/src/main.rs and crates/laqta-ml/src/install.rs, so the
  # app uses these instead of downloading prebuilt copies that do not run on NixOS.
  env = {
    LAQTA_TOOLS_PATH = lib.makeBinPath tools;
  }
  // lib.optionalAttrs withOnnxruntime {
    LAQTA_ONNXRUNTIME = "${lib.getLib onnxruntime}/lib/libonnxruntime.so";
  };

  # The suite needs a display, a portal session and network hosts; run it with `cargo test` in
  # `nix develop`.
  doCheck = false;

  postInstall = ''
    install -Dm644 packaging/linux/io.github.etherll.Laqta.desktop -t $out/share/applications
    substituteInPlace $out/share/applications/io.github.etherll.Laqta.desktop \
      --replace-fail "Exec=/usr/bin/laqta" "Exec=laqta"
    for size in 16 32 64 128 256 512; do
      install -Dm644 assets/icon/laqta-$size.png \
        $out/share/icons/hicolor/''${size}x''${size}/apps/io.github.etherll.Laqta.png
    done
    install -Dm644 assets/icon/laqta.svg -T $out/share/icons/hicolor/scalable/apps/io.github.etherll.Laqta.svg
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath dlopenLibs} $out/bin/laqta
  '';

  passthru = { inherit ffmpeg; };

  meta = {
    description = "Capture, record, mark up and share your screen";
    homepage = "https://github.com/Etherll/laqta";
    license = lib.licenses.gpl3Plus;
    mainProgram = "laqta";
    platforms = lib.platforms.linux;
  };
}
