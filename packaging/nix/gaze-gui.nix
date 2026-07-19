# GTK4/Adwaita GUI for Gaze (mirrors packaging/nfpm-gui.yaml).
{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook4,
  glib,
  gst_all_1,
  gtk4,
  libadwaita,
  opencv,
  openssl,
  pipewire,
}:

let
  gstPluginPath = lib.makeSearchPath "lib/gstreamer-1.0" [
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
    pipewire
  ];
in
rustPlatform.buildRustPackage {
  pname = "gaze-gui";
  version = (builtins.fromTOML (builtins.readFile ../../gaze-gui/Cargo.toml)).package.version;

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../gaze
      ../../gaze-cli
      ../../gaze-core
      ../../gaze-gui
      ../../pam-gaze
      ../../pam-gaze-core
      ../../pam-gaze-grosshack
      ../../packaging/gui
    ];
  };

  cargoLock.lockFile = ../../Cargo.lock;

  # Keeps gaze-core's `detection` feature (ONNX Runtime) out of the GUI.
  cargoBuildFlags = [
    "--package"
    "gaze-gui"
  ];

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
    # The opencv crate generates its bindings with libclang at build time.
    rustPlatform.bindgenHook
  ];

  buildInputs = [
    glib
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gtk4
    libadwaita
    opencv
    openssl
  ];

  # The test suite expects a camera and a running system bus.
  doCheck = false;

  postInstall = ''
    install -Dm644 packaging/gui/com.gundulabs.Gaze.desktop $out/share/applications/com.gundulabs.Gaze.desktop
    install -Dm644 packaging/gui/com.gundulabs.Gaze.svg $out/share/icons/hicolor/scalable/apps/com.gundulabs.Gaze.svg
    install -Dm644 packaging/gui/com.gundulabs.Gaze.metainfo.xml $out/share/metainfo/com.gundulabs.Gaze.metainfo.xml
  '';

  preFixup = ''
    gappsWrapperArgs+=(--prefix GST_PLUGIN_SYSTEM_PATH_1_0 : "${gstPluginPath}")
  '';

  meta = {
    description = "GTK4/Adwaita GUI for Gaze facial authentication";
    homepage = "https://gaze.gundulabs.com";
    license = lib.licenses.mit;
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
    mainProgram = "gaze-gui";
  };
}
