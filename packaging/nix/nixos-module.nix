# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# NixOS module for Gaze: daemon, D-Bus/polkit, PAM, GUI, and GNOME wiring.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.gaze;

  settingsFormat = pkgs.formats.toml { };

  # Merge user `settings` over the upstream default config.
  defaultSettings = builtins.fromTOML (builtins.readFile ../config/config.toml);
  userSecuritySettings = cfg.settings.security or { };
  legacySecurityThreshold = userSecuritySettings.threshold or null;
  normalizedSecuritySettings =
    (removeAttrs userSecuritySettings [ "threshold" ])
    // lib.optionalAttrs (legacySecurityThreshold != null) {
      rgb_threshold = userSecuritySettings.rgb_threshold or legacySecurityThreshold;
      ir_threshold = userSecuritySettings.ir_threshold or legacySecurityThreshold;
    };
  normalizedSettings = cfg.settings // lib.optionalAttrs (cfg.settings ? security) {
    security = normalizedSecuritySettings;
  };
  configFile = settingsFormat.generate "gaze-config.toml" (
    lib.recursiveUpdate defaultSettings normalizedSettings
  );

  pamModuleFor =
    svc: "${cfg.package}/lib/security/pam_gaze${lib.optionalString svc.simultaneous "_grosshack"}.so";

  gnomeExtensionUuid = cfg.gnome.extensionPackage.passthru.extensionUuid;
in
{
  options.services.gaze = {
    enable = lib.mkEnableOption "Gaze face authentication (gazed daemon, CLI, and PAM modules)";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The gaze package providing gazed, the CLI, and the PAM modules.";
      defaultText = lib.literalExpression ''gaze.packages.''${system}.gaze'';
    };

    settings = lib.mkOption {
      type = settingsFormat.type;
      default = { };
      example = lib.literalExpression ''
        {
          security.level = "high";
          auth.require_confirmation_lock_screen = true;
        }
      '';
      description = ''
        Options for {file}`/etc/gaze/config.toml`, merged over the upstream
        defaults. See <https://gaze.gundulabs.com/guide/configuration>.
      '';
    };

    mutableConfig = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        If true, {file}`/etc/gaze/config.toml` is seeded from `settings` on
        first boot but stays editable afterwards (the GUI's settings page
        writes to it), matching the `noreplace` behaviour of the deb/rpm
        packages; later changes to `settings` do not overwrite an existing
        file. If false, the config is a read-only symlink managed entirely
        by `settings` and edits from the GUI fail.
      '';
    };

    pam.defaultServices = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [
        "sudo"
        "polkit-1"
      ];
      example = [
        "sudo"
        "polkit-1"
        "login"
      ];
      description = ''
        PAM services that get face authentication without any further
        configuration, matching what the deb, rpm, and Arch packages set up on
        install. Each entry only defaults
        {option}`security.pam.services.<name>.gaze.enable` to true, so an
        individual service can still be turned off with
        `security.pam.services.sudo.gaze.enable = false`.
      '';
    };

    tpm.tcti = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "device:/dev/tpm0";
      description = ''
        TCTI connection string for sealing template keys, exported to the
        daemon as `TPM2TOOLS_TCTI`. When null, the daemon prefers
        {file}`/dev/tpmrm0` and falls back to {file}`/dev/tpm0` when the
        resource-manager node is missing or unreadable. Set this only if
        neither node is the one you want, for example to reach a TPM through
        `tabrmd`.
      '';
    };

    gui = {
      enable = lib.mkEnableOption "the Gaze GTK4/Adwaita configuration GUI";

      package = lib.mkOption {
        type = lib.types.package;
        description = "The gaze-gui package.";
        defaultText = lib.literalExpression ''gaze.packages.''${system}.gaze-gui'';
      };
    };

    gnome = {
      enable = lib.mkEnableOption ''
        the Gaze GNOME Shell extension (lock screen face unlock). Turned on for
        every GNOME session by default, see
        {option}`services.gaze.gnome.enableForUsers`
      '';

      extensionPackage = lib.mkOption {
        type = lib.types.package;
        description = "The gaze-gnome-extension package.";
        defaultText = lib.literalExpression ''gaze.packages.''${system}.gaze-gnome-extension'';
      };

      enableForUsers = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Load the extension and turn on lock screen face auth in every GNOME
          user session, by writing dconf defaults into the `user` profile.
          Without this the extension is only installed, and each user has to
          turn it on from the Extensions app before face unlock works.

          These are defaults rather than locks, so a user who switches the
          extension off in the Extensions app, or face auth off in the
          extension preferences, keeps that choice.

          Set this to `false` if you manage `org/gnome/shell`
          `enabled-extensions` yourself, through home-manager or your own
          {option}`programs.dconf.profiles.user` database. Two system databases
          that both set the key shadow each other instead of merging, and only
          the one listed first in the profile takes effect.
        '';
      };

      gdmFaceLogin = lib.mkEnableOption ''
        face authentication at the GDM login screen itself (not just the lock
        screen). Keep password login working and read
        <https://gaze.gundulabs.com/guide/gnome> before enabling
      '';
    };
  };

  # Per-service knobs live on the nixpkgs PAM submodule, the way `fprintAuth`
  # and `howdy` do. Only the type may be declared a second time: a `default`,
  # `example`, or `description` here would collide with nixpkgs'.
  options.security.pam.services = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule (
        { name, config, ... }:
        {
          options.gaze = {
            enable = lib.mkOption {
              type = lib.types.bool;
              default = cfg.enable && lib.elem name cfg.pam.defaultServices;
              defaultText = lib.literalExpression ''
                config.services.gaze.enable
                && lib.elem name config.services.gaze.pam.defaultServices
              '';
              description = ''
                Whether to attempt face authentication for this PAM service.
                The rule is inserted ahead of `pam_fprintd` and `pam_unix`, so
                face auth runs first and the fingerprint reader and password
                both remain fallbacks.
              '';
            };

            control = lib.mkOption {
              type = lib.types.str;
              default = "sufficient";
              description = "PAM control field for the gaze auth rule.";
            };

            simultaneous = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = ''
                Use pam_gaze_grosshack.so, which runs face authentication and
                the password prompt simultaneously, instead of the sequential
                pam_gaze.so.
              '';
            };

            order = lib.mkOption {
              type = lib.types.nullOr lib.types.int;
              default = null;
              example = lib.literalExpression ''
                config.security.pam.services.login.rules.auth.unix.order + 10
              '';
              description = ''
                Explicit `order` for the gaze auth rule. When null, the rule is
                placed ahead of both `pam_fprintd` and `pam_unix`. Set this to
                reorder gaze relative to another rule, offsetting from that
                rule's `order` rather than from a constant. Note that `rules`
                is an experimental nixpkgs option whose numbering can change
                between releases.
              '';
            };
          };

          config.rules.auth.gaze = lib.mkIf config.gaze.enable (
            let
              authRules = config.rules.auth;
              # Ahead of both fingerprint and password: nixpkgs auto-assigns
              # these orders, so offset from them instead of hardcoding a value.
              fallbackOrder = lib.min authRules.unix.order (
                authRules.fprintd.order or authRules.unix.order
              );
            in
            {
              control = config.gaze.control;
              modulePath = pamModuleFor config.gaze;
              order = if config.gaze.order != null then config.gaze.order else fallbackOrder - 10;
            }
          );
        }
      )
    );
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        environment.systemPackages = [ cfg.package ];

        # Ships the com.gundulabs.Gaze system bus policy.
        services.dbus.packages = [ cfg.package ];

        # Enrollment and configuration changes are authorized via polkit.
        security.polkit.enable = true;

        systemd.services.gazed = {
          description = "Daemon for Gaze";
          # Mirrors packaging/config/gazed.service.
          after = [ "dbus.service" ];
          requires = [ "dbus.service" ];
          wantedBy = [ "multi-user.target" ];
          environment = {
            XDG_CACHE_HOME = "/var/cache/gaze";
          }
          // lib.optionalAttrs (cfg.tpm.tcti != null) {
            TPM2TOOLS_TCTI = cfg.tpm.tcti;
          };
          serviceConfig = {
            ExecStart = "${cfg.package}/bin/gazed";
            Restart = "on-failure";
            RestartSec = 5;
            StateDirectory = "gaze";
            StateDirectoryMode = "0700";
            CacheDirectory = "gaze";
            UMask = "0077";
            LimitCORE = 0;
            NoNewPrivileges = true;
            PrivateTmp = true;
            ProtectSystem = "strict";
            InaccessiblePaths = [
              "/home"
              "/root"
            ];
            ProtectClock = true;
            ProtectHostname = true;
            ProtectKernelTunables = true;
            ProtectKernelModules = true;
            ProtectControlGroups = true;
            ProtectKernelLogs = true;
            RestrictNamespaces = true;
            RestrictRealtime = true;
            RestrictSUIDSGID = true;
            LockPersonality = true;
            SystemCallArchitectures = "native";
            CapabilityBoundingSet = [
              "CAP_DAC_READ_SEARCH"
              "CAP_DAC_OVERRIDE"
            ];
            # The GUI's settings page rewrites config.toml through the daemon,
            # and the GDM face-login toggle writes a dconf database.
            ReadWritePaths = [
              "-/etc/gaze"
              "-/etc/dconf/db"
            ];
            # IR cameras need read/write access to their /dev/video* node, and
            # sealing template keys needs the TPM resource manager.
            SupplementaryGroups =
              [ "video" ]
              ++ lib.optional config.security.tpm2.enable config.security.tpm2.tssGroup;
          };
        };
      }

      {
        # mkIf must stay at the value level here to avoid infinite recursion.
        systemd.tmpfiles.rules = lib.mkIf cfg.mutableConfig [
          "d /etc/gaze 0755 root root -"
          "C /etc/gaze/config.toml 0644 root root - ${configFile}"
        ];
        environment.etc."gaze/config.toml" = lib.mkIf (!cfg.mutableConfig) {
          source = configFile;
        };
      }

      (lib.mkIf cfg.gui.enable {
        environment.systemPackages = [ cfg.gui.package ];
      })

      (lib.mkIf cfg.gnome.enable {
        environment.systemPackages = [ cfg.gnome.extensionPackage ];
        # The daemon reads the GDM face-login state out of the gdm dconf profile.
        systemd.services.gazed.path = [ pkgs.dconf ];
        # Expose the extension to GNOME Shell (and to the GDM greeter).
        environment.pathsToLink = [ "/share/gnome-shell" ];

        # Equivalent of packaging/gdm/00-gaze-defaults.
        programs.dconf.enable = true;
        programs.dconf.profiles =
          {
            gdm.databases = [
              {
                settings = {
                  "org/gnome/shell".enabled-extensions = [ gnomeExtensionUuid ];
                  "org/gnome/shell/extensions/gaze".enable-face-authentication =
                    cfg.gnome.gdmFaceLogin;
                };
              }
            ];
          }
          // lib.optionalAttrs cfg.gnome.enableForUsers {
            user.databases = [
              {
                settings = {
                  "org/gnome/shell".enabled-extensions = [ gnomeExtensionUuid ];
                  "org/gnome/shell/extensions/gaze".enable-face-authentication = true;
                };
              }
            ];
          };

        # Face-only PAM service, equivalent of packaging/pam/gdm-face.arch.
        security.pam.services."gdm-face".text = ''
          auth       required                     pam_env.so
          auth       [success=done ignore=ignore default=bad]   ${cfg.package}/lib/security/pam_gaze.so
          auth       optional                     ${pkgs.gnome-keyring}/lib/security/pam_gnome_keyring.so only_if=login auto_start
          auth       required                     pam_deny.so

          account    required                     pam_nologin.so
          account    required                     pam_unix.so

          password   required                     pam_deny.so

          session    required                     pam_env.so conffile=/etc/pam/environment readenv=0
          session    required                     pam_unix.so
          session    required                     ${config.systemd.package}/lib/security/pam_systemd.so
          session    optional                     ${pkgs.gnome-keyring}/lib/security/pam_gnome_keyring.so auto_start
        '';
      })
    ]
  );
}
