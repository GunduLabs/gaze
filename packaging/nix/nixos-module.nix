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
  configFile = settingsFormat.generate "gaze-config.toml" (
    lib.recursiveUpdate defaultSettings cfg.settings
  );

  pamModuleFor =
    svc: "${cfg.package}/lib/security/pam_gaze${lib.optionalString svc.simultaneous "_grosshack"}.so";
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
          auth.require_confirmation = true;
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

    pam = {
      services = lib.mkOption {
        type = lib.types.attrsOf (
          lib.types.submodule {
            options = {
              control = lib.mkOption {
                type = lib.types.str;
                default = "sufficient";
                description = "PAM control field for the gaze auth rule.";
              };
              simultaneous = lib.mkOption {
                type = lib.types.bool;
                default = false;
                description = ''
                  Use pam_gaze_grosshack.so, which runs face authentication
                  and the password prompt simultaneously, instead of the
                  sequential pam_gaze.so.
                '';
              };
              order = lib.mkOption {
                type = lib.types.nullOr lib.types.int;
                default = null;
                example = lib.literalExpression ''
                  config.security.pam.services.login.rules.auth.unix.order + 10
                '';
                description = ''
                  Explicit `order` for the gaze auth rule. When null, the rule
                  is placed ahead of both `pam_fprintd` and `pam_unix`, so face
                  authentication is tried first and the fingerprint reader and
                  password both remain fallbacks. Set this to reorder gaze
                  relative to another rule, offsetting from that rule's `order`
                  rather than from a constant.
                '';
              };
            };
          }
        );
        default = {
          sudo = { };
          polkit-1 = { };
        };
        example = lib.literalExpression ''
          {
            sudo = { };
            polkit-1 = { };
            hyprlock.simultaneous = true;
          }
        '';
        description = ''
          PAM services to enable face authentication for. The gaze rule is
          inserted ahead of `pam_fprintd` and `pam_unix`, so face auth is
          attempted first and the password remains a fallback. The default
          matches what the deb, rpm, and Arch packages configure on install.
        '';
      };
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
        the Gaze GNOME Shell extension (lock screen face unlock). Users still
        have to turn it on per user with
        `gnome-extensions enable gaze@gundulabs.com`
      '';

      extensionPackage = lib.mkOption {
        type = lib.types.package;
        description = "The gaze-gnome-extension package.";
        defaultText = lib.literalExpression ''gaze.packages.''${system}.gaze-gnome-extension'';
      };

      gdmFaceLogin = lib.mkEnableOption ''
        face authentication at the GDM login screen itself (not just the lock
        screen). Keep password login working and read
        <https://gaze.gundulabs.com/guide/gnome> before enabling
      '';
    };
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
          serviceConfig = {
            ExecStart = "${cfg.package}/bin/gazed";
            Restart = "on-failure";
            RestartSec = 5;
            StateDirectory = "gaze";
            StateDirectoryMode = "0700";
            CacheDirectory = "gaze";
            UMask = "0077";
            NoNewPrivileges = true;
            PrivateTmp = true;
            ProtectSystem = "strict";
            InaccessiblePaths = [
              "/home"
              "/root"
            ];
            ProtectKernelTunables = true;
            ProtectKernelModules = true;
            ProtectControlGroups = true;
            RestrictSUIDSGID = true;
            LockPersonality = true;
            SystemCallArchitectures = "native";
            CapabilityBoundingSet = [ "CAP_DAC_READ_SEARCH" ];
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

        security.pam.services = lib.mapAttrs (
          name: svc:
          let
            authRules = config.security.pam.services.${name}.rules.auth;
            # Ahead of both fingerprint and password: nixpkgs auto-assigns
            # these orders, so offset from them instead of hardcoding a value.
            fallbackOrder = lib.min authRules.unix.order (
              authRules.fprintd.order or authRules.unix.order
            );
          in
          {
            rules.auth.gaze = {
              control = svc.control;
              modulePath = pamModuleFor svc;
              order = if svc.order != null then svc.order else fallbackOrder - 10;
            };
          }
        ) cfg.pam.services;
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
        # Expose the extension to GNOME Shell (and to the GDM greeter).
        environment.pathsToLink = [ "/share/gnome-shell" ];

        # Equivalent of packaging/gdm/00-gaze-defaults.
        programs.dconf.enable = true;
        programs.dconf.profiles.gdm.databases = [
          {
            settings = {
              "org/gnome/shell".enabled-extensions = [
                cfg.gnome.extensionPackage.passthru.extensionUuid
              ];
              "org/gnome/shell/extensions/gaze".enable-face-authentication = cfg.gnome.gdmFaceLogin;
            };
          }
        ];

        # Face-only PAM service, equivalent of packaging/pam/gdm-face.arch.
        security.pam.services."gdm-face".text = ''
          auth       required                     pam_env.so
          auth       [success=done default=bad]   ${cfg.package}/lib/security/pam_gaze.so
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
