# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Cinnamon Spices extension for Gaze (mirrors packaging/nfpm-cinnamon-extension.yaml).
{
  lib,
  stdenvNoCC,
}:

let
  uuid = "gaze@gundulabs.com";
in
stdenvNoCC.mkDerivation {
  pname = "gaze-cinnamon-extension";
  version = (builtins.fromTOML (builtins.readFile ../../gaze/Cargo.toml)).package.version;

  src = lib.fileset.toSource {
    root = ../..;
    fileset = ../../cinnamon-extension;
  };

  installPhase = ''
    runHook preInstall

    ext=$out/share/cinnamon/extensions/${uuid}
    install -Dm644 cinnamon-extension/metadata.json -t "$ext"
    install -Dm644 cinnamon-extension/extension.js -t "$ext"
    install -Dm644 cinnamon-extension/settings-schema.json -t "$ext"

    runHook postInstall
  '';

  passthru.extensionUuid = uuid;

  meta = {
    description = "Cinnamon Spices extension for Gaze facial authentication";
    homepage = "https://gaze.gundulabs.com";
    license = lib.licenses.gpl3Plus;
    platforms = lib.platforms.linux;
  };
}
