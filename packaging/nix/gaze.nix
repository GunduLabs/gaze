# Daemon, CLI, and PAM modules for Gaze (mirrors packaging/nfpm.yaml).
{
  lib,
  rustPlatform,
  pkg-config,
  makeWrapper,
  glib,
  gst_all_1,
  onnxruntime,
  opencv,
  openssl,
  pipewire,
  tpm2-tss,
}:

let
  # Runtime GStreamer plugins (v4l2src, pipewiresrc).
  gstPluginPath = lib.makeSearchPath "lib/gstreamer-1.0" [
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
    pipewire
  ];
in
rustPlatform.buildRustPackage {
  pname = "gaze";
  version = (builtins.fromTOML (builtins.readFile ../../gaze/Cargo.toml)).package.version;

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
      ../../packaging/config
    ];
  };

  cargoLock.lockFile = ../../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
    # The opencv crate generates its bindings with libclang at build time.
    rustPlatform.bindgenHook
  ];

  buildInputs = [
    glib
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    onnxruntime
    opencv
    openssl
    tpm2-tss
  ];

  env = {
    # Link the nixpkgs ONNX Runtime instead of letting ort download one.
    ORT_STRATEGY = "system";
    ORT_LIB_LOCATION = "${lib.getLib onnxruntime}/lib";
    ORT_PREFER_DYNAMIC_LINK = "1";
  };

  # Two invocations keep gaze-core's `detection` feature out of the clients.
  buildPhase = ''
    runHook preBuild
    cargo build --release --offline -p gaze
    cargo build --release --offline -p gaze-cli -p pam-gaze -p pam-gaze-grosshack
    runHook postBuild
  '';

  # The test suite expects a camera and a running system bus.
  doCheck = false;

  installPhase = ''
    runHook preInstall
    install -Dm755 target/release/gazed $out/bin/gazed
    install -Dm755 target/release/gaze $out/bin/gaze
    install -Dm755 target/release/libpam_gaze.so $out/lib/security/pam_gaze.so
    install -Dm755 target/release/libpam_gaze_grosshack.so $out/lib/security/pam_gaze_grosshack.so
    install -Dm644 packaging/config/config.toml $out/share/gaze/config.toml
    install -Dm644 packaging/config/com.gundulabs.Gaze.conf $out/share/dbus-1/system.d/com.gundulabs.Gaze.conf
    install -Dm644 packaging/config/com.gundulabs.gaze.policy $out/share/polkit-1/actions/com.gundulabs.gaze.policy
    runHook postInstall
  '';

  postFixup = ''
    wrapProgram $out/bin/gazed \
      --prefix GST_PLUGIN_SYSTEM_PATH_1_0 : "${gstPluginPath}"
    wrapProgram $out/bin/gaze \
      --prefix GST_PLUGIN_SYSTEM_PATH_1_0 : "${gstPluginPath}"
  '';

  meta = {
    description = "Daemon, CLI, and PAM integration for Gaze facial authentication";
    homepage = "https://gaze.gundulabs.com";
    license = lib.licenses.mit;
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
    mainProgram = "gaze";
  };
}
