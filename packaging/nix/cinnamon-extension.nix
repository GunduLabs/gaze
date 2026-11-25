# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Cinnamon Spices extension for Gaze (mirrors packaging/nfpm/cinnamon-extension.yaml).
{
  lib,
  stdenvNoCC,
  gettext,
}:

let
  uuid = "gaze@gundulabs.com";
in
stdenvNoCC.mkDerivation {
  pname = "gaze-cinnamon-extension";
  version = (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).workspace.package.version;

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../integrations/cinnamon
      ../../scripts/build-locales.sh
    ];
  };

  nativeBuildInputs = [ gettext ];

  installPhase = ''
    runHook preInstall

    ext=$out/share/cinnamon/extensions/${uuid}
    install -Dm644 integrations/cinnamon/metadata.json -t "$ext"
    install -Dm644 integrations/cinnamon/extension.js -t "$ext"
    install -Dm644 integrations/cinnamon/settings-schema.json -t "$ext"
    bash scripts/build-locales.sh integrations/cinnamon/po "$out/share/locale"

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
