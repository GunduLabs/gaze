# GNOME Shell extension for Gaze (mirrors packaging/nfpm-gnome-extension.yaml).
# GDM defaults and the gdm-face PAM service live in the NixOS module.
{
  lib,
  stdenvNoCC,
  glib,
}:

let
  uuid = "gaze@gundulabs.com";
in
stdenvNoCC.mkDerivation {
  pname = "gaze-gnome-extension";
  version = (builtins.fromTOML (builtins.readFile ../../gaze/Cargo.toml)).package.version;

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../gnome-shell-extension
      ../../packaging/config/org.gnome.shell.extensions.gaze.gschema.xml
    ];
  };

  nativeBuildInputs = [ glib ];

  installPhase = ''
    runHook preInstall

    ext=$out/share/gnome-shell/extensions/${uuid}
    install -Dm644 gnome-shell-extension/metadata.json -t "$ext"
    install -Dm644 gnome-shell-extension/extension.js -t "$ext"
    install -Dm644 gnome-shell-extension/prefs.js -t "$ext"

    # GNOME Shell loads the settings schema from the extension directory.
    install -Dm644 packaging/config/org.gnome.shell.extensions.gaze.gschema.xml \
      -t "$ext/schemas"
    glib-compile-schemas "$ext/schemas"

    schemadir=$out/share/gsettings-schemas/gaze-gnome-extension/glib-2.0/schemas
    install -Dm644 packaging/config/org.gnome.shell.extensions.gaze.gschema.xml -t "$schemadir"
    glib-compile-schemas "$schemadir"

    runHook postInstall
  '';

  passthru.extensionUuid = uuid;

  meta = {
    description = "GNOME Shell extension for Gaze facial authentication";
    homepage = "https://gaze.gundulabs.com";
    license = lib.licenses.gpl3Plus;
    platforms = lib.platforms.linux;
  };
}
