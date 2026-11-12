{
  lib,
  craneLib,
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
  # The Cargo profile. `dist` is what users download (Cargo.toml): fat LTO, slower to build and
  # smaller to download. CI builds pull requests with `release`, which only has to prove it builds.
  cargoProfile ? "dist",
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

  # Shared by the dependency build and the package build. Anything here that changes rebuilds
  # every dependency, so it holds only what compiling them needs.
  commonArgs = {
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

    strictDeps = true;
    CARGO_PROFILE = cargoProfile;
    cargoExtraArgs = "--locked --package=laqta --package=laqta-nmh";

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

    # The suite needs a display, a portal session and network hosts; run it with `cargo test` in
    # `nix develop`.
    doCheck = false;
  };

  # The crates `[patch.crates-io]` swaps in from vendor/ are dependencies, so they keep their real
  # sources; crane would otherwise stub them out with Laqta's own crates.
  vendored = lib.fileset.toSource {
    root = ../.;
    fileset = ../vendor;
  };

  # Every dependency compiled, with Laqta's own crates stubbed out. Its hash follows the manifests,
  # `Cargo.lock` and vendor/, so CI's Cachix copy serves every build that leaves them alone.
  cargoArtifacts = craneLib.buildDepsOnly (
    builtins.removeAttrs commonArgs [ "src" ]
    // {
      # From the real sources: reading Cargo.lock out of the stubbed ones would build them while
      # the flake is evaluated.
      cargoVendorDir = craneLib.vendorCargoDeps { inherit (commonArgs) src; };
      dummySrc = craneLib.mkDummySrc (
        commonArgs
        // {
          extraDummyScript = ''
            rm -rf $out/vendor
            cp -r --no-preserve=mode,ownership ${vendored}/vendor $out/vendor
          '';
        }
      );
      # The package build runs no `cargo check`, so its artifacts would go unused.
      cargoCheckCommand = "true";
    }
  );
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;

    # Read with option_env! in apps/laqta/src/main.rs and crates/laqta-ml/src/install.rs, so the
    # app uses these instead of downloading prebuilt copies that do not run on NixOS.
    LAQTA_TOOLS_PATH = lib.makeBinPath tools;
  }
  // lib.optionalAttrs withOnnxruntime {
    LAQTA_ONNXRUNTIME = "${lib.getLib onnxruntime}/lib/libonnxruntime.so";
  }
  // {
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

    passthru = { inherit ffmpeg cargoArtifacts dlopenLibs; };

    meta = {
      description = "Capture, record, mark up and share your screen";
      homepage = "https://github.com/Etherll/laqta";
      license = lib.licenses.gpl3Plus;
      mainProgram = "laqta";
      platforms = lib.platforms.linux;
    };
  }
)
