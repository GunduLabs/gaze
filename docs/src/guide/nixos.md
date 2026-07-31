# Nix & NixOS

Gaze ships a Nix flake with packages for the daemon, CLI, GUI, and GNOME Shell
extension, plus a NixOS module that wires up everything the distro packages do
imperatively: the `gazed` systemd service, D-Bus and polkit policies, PAM
integration, and GNOME/GDM defaults.

Face authentication needs a system daemon and PAM configuration, so the NixOS
module is the recommended path. On non-NixOS systems with Nix (including
home-manager), you can install the CLI and GUI from the flake, but the daemon
and PAM modules must come from your distro's Gaze package, the same split as
the Flatpak.

## Flake outputs

| Output | Description |
| --- | --- |
| `packages.<system>.gaze` | `gazed` daemon, `gaze` CLI, and the PAM modules |
| `packages.<system>.gaze-gui` | GTK4/Adwaita configuration GUI |
| `packages.<system>.gaze-gnome-extension` | GNOME Shell extension |
| `nixosModules.default` | NixOS module (`services.gaze.*`) |
| `overlays.default` | Adds the three packages to `pkgs` |
| `devShells.<system>.default` | Development shell with the full build environment |

`<system>` is `x86_64-linux` or `aarch64-linux`.

## NixOS

Add the flake input and import the module:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    gaze = {
      url = "github:GunduLabs/gaze";
      # Optional, saves a second nixpkgs evaluation. Only do this if your
      # nixpkgs is unstable: Gaze is edition 2024, and a stable channel's
      # older rustc fails partway through the dependency tree (`kstring`,
      # pulled in by the gstreamer crate, is usually the first to break).
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, gaze, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        gaze.nixosModules.default
        {
          services.gaze = {
            enable = true;
            gui.enable = true;
          };

          # `sudo` and `polkit-1` attempt face auth out of the box, with the
          # password still available as a fallback. Other PAM services opt in
          # one at a time, e.g.:
          # security.pam.services.hyprlock.gaze.enable = true;
        }
        # ... the rest of your configuration
      ];
    };
  };
}
```

Then rebuild and enroll:

```bash
sudo nixos-rebuild switch
gaze add-face default
gaze auth --verbose
```

The recognition models are downloaded by the daemon into `/var/cache/gaze` on
first use, exactly as on other distros.

::: warning ONNX Runtime version
The `ort` crate used by `gazed` is built against the `onnxruntime` package
from the nixpkgs revision the flake is built with. Upstream tests against
ONNX Runtime 1.22; if your nixpkgs ships a very different version and the
daemon fails to start, pin the flake's `nixpkgs` input to a revision with a
compatible `onnxruntime` instead of overriding `follows`.
:::

### Module options

| Option | Default | Description |
| --- | --- | --- |
| `services.gaze.enable` | `false` | Run `gazed` and install the CLI and PAM modules |
| `services.gaze.package` | flake's `gaze` | Daemon/CLI/PAM package |
| `services.gaze.settings` | `{ }` | Options merged over the upstream defaults into `/etc/gaze/config.toml` (see [Configuration](/guide/configuration)) |
| `services.gaze.mutableConfig` | `true` | Seed `/etc/gaze/config.toml` once and leave it editable (the GUI writes to it). Set to `false` for a fully declarative, read-only config |
| `services.gaze.pam.defaultServices` | `[ "sudo" "polkit-1" ]` | PAM services that get face auth without further configuration |
| `security.pam.services.<name>.gaze.enable` | true for `pam.defaultServices` | Attempt face auth for this PAM service |
| `security.pam.services.<name>.gaze.control` | `"sufficient"` | PAM control field for the rule |
| `security.pam.services.<name>.gaze.simultaneous` | `false` | Use `pam_gaze_grosshack.so` (face and password prompt at the same time) instead of sequential `pam_gaze.so` |
| `security.pam.services.<name>.gaze.order` | `null` | Explicit rule `order`; `null` places gaze ahead of `pam_fprintd` and `pam_unix` |
| `services.gaze.gui.enable` | `false` | Install `gaze-gui` |
| `services.gaze.gnome.enable` | `false` | Install the GNOME Shell extension and the `gdm-face` PAM service |
| `services.gaze.gnome.gdmFaceLogin` | `false` | Also enable face auth at the GDM login screen (read the [GNOME guide](/guide/gnome) first) |
| `services.gaze.kde.lockScreen` | `false` | Define `kde-fingerprint` so the Plasma lock screen starts face unlock with no key press |
| `services.gaze.kde.loginScreen` | `false` | Also run face auth in the Plasma Login Manager / SDDM stack (submit-driven; see the [KDE guide](/guide/kde)) |
| `services.gaze.tpm.tcti` | `null` | `TPM2TOOLS_TCTI` for the daemon, e.g. `"device:/dev/tpm0"`. Only needed when neither `/dev/tpmrm0` nor `/dev/tpm0` is the right node |

The gaze PAM rule is inserted ahead of both `pam_fprintd` and `pam_unix`, so
face auth is tried first and the fingerprint reader and password both remain
fallbacks. To put gaze somewhere else in the stack, set `order`, offsetting
from the neighbouring rule rather than from a constant (nixpkgs assigns those
numbers automatically and they change between releases):

```nix
# Try the fingerprint reader first, then gaze, then the password.
security.pam.services.login.gaze = {
  enable = true;
  order = config.security.pam.services.login.rules.auth.fprintd.order + 5;
};
```

::: warning `rules` is experimental
`security.pam.services.<name>.rules` is an experimental nixpkgs option that
can change without notice, and the `order` numbers of the built-in rules move
between releases. Always offset from a neighbouring rule as above instead of
assigning a constant, or a nixpkgs update can silently reorder your stack.
:::

For anything the options don't cover, use
`security.pam.services.<name>.rules` directly with
`${config.services.gaze.package}/lib/security/pam_gaze.so`.

::: warning `settings` is applied once
With the default `mutableConfig = true`, `/etc/gaze/config.toml` is seeded on
first activation and never overwritten again, so later changes to
`services.gaze.settings` have no effect on a machine that already has the
file. Either edit `/etc/gaze/config.toml` (or use the GUI), delete it and
rebuild to re-seed it, or set `mutableConfig = false` for a fully declarative
config.
:::

### Cameras

Leave `settings.cameras.rgb` unset unless you have a specific reason to pin a
camera: the default, `"primary"`, resolves the primary color camera at
runtime. A pinned value must be a PipeWire node identity, not a device path;
`pipewiresrc target-object=/dev/video0` does not match anything. Run
`gaze doctor` to see the sources PipeWire currently advertises, and copy one
of those strings verbatim.

### GNOME lock screen

```nix
services.gaze.gnome.enable = true;
```

Then, from your GNOME session, enable the extension for your user and turn on
lock screen face auth in its preferences:

```bash
gnome-extensions enable gaze@gundulabs.com
gnome-extensions prefs gaze@gundulabs.com
```

The extension's settings schema is installed inside the extension directory
rather than into the system schema path, so a bare
`gsettings set org.gnome.shell.extensions.gaze …` cannot find it. If you want
to set the key from a script, write it through dconf instead, which does not
need the schema:

```bash
dconf write /org/gnome/shell/extensions/gaze/enable-face-authentication true
```

Log out and back in once if the lock screen does not pick it up immediately.
GDM *login* face auth stays disabled unless you set
`services.gaze.gnome.gdmFaceLogin = true;`. The extension preferences and
`gaze doctor` both report whatever that option compiled into the GDM dconf
profile, but the toggle itself cannot change it: the profile belongs to your
NixOS configuration, so switching it there is the only way.

### hyprlock

```nix
programs.hyprlock.enable = true;
security.pam.services.hyprlock.gaze.enable = true;
# or, for simultaneous face + password:
# security.pam.services.hyprlock.gaze = { enable = true; simultaneous = true; };
```

This modifies the `hyprlock` PAM service in place, so no `auth_pam_module`
changes in `hyprlock.conf` are needed. See the [Hyprland guide](/guide/hyprland)
for behavior details.

### KDE Plasma

```nix
services.gaze.kde.lockScreen = true;
```

That defines the `kde-fingerprint` PAM service, the slot KScreenLocker starts up
front for biometrics, so face unlock begins with no key press. It also turns
`security.pam.services.kde.gaze.enable` off, because the greeter runs the
interactive `kde` service at the same time and two clients would fight over one
camera. Don't set `kde.gaze.enable` by hand: that is the old submit-driven wiring
and it conflicts with this one.

The login greeter is separate and opt-in, because upstream gives it no up-front
biometric slot, so face auth only runs when the login form is submitted:

```nix
services.gaze.kde.loginScreen = true;
```

KWallet will ask for its password once per session after a face login, since
there was no password to hand it. See the [KDE Plasma guide](/guide/kde).

The System Settings entry ships in the `gaze-kde` package rather than the NixOS
module, so on Nix use `gaze add-face` or `services.gaze.gui.enable`.

### Other desktops

Add the relevant PAM service names, e.g.:

```nix
security.pam.services.login.gaze.enable = true; # console/display-manager login
```

### Uninstalling

Don't use `gaze uninstall` on NixOS; it drives distro package managers.
Remove the module (or set `services.gaze.enable = false;`), rebuild, and
delete the leftover state if you want a clean slate:

```bash
sudo rm -rf /var/lib/gaze /var/cache/gaze /etc/gaze
```

## Nix without NixOS, and home-manager

The packages work with plain `nix profile` and with home-manager:

```bash
nix profile install github:GunduLabs/gaze#gaze-gui
```

```nix
# home-manager
home.packages = [
  inputs.gaze.packages.${pkgs.stdenv.hostPlatform.system}.gaze-gui
];
```

These talk to `gazed` over the system bus, so the daemon and PAM integration
must already be installed system-wide, either via the NixOS module or via
your distro's Gaze package ([Installation](/guide/installation)).
Installing only the flake packages on a non-NixOS system does **not** set up
the daemon, its D-Bus/polkit policies, or PAM.

## Development shell

```bash
nix develop github:GunduLabs/gaze
# or, in a checkout:
nix develop
```

The shell provides the Rust toolchain and all native build inputs (OpenCV,
GStreamer, GTK4, ONNX Runtime, tpm2-tss) with the `ORT_STRATEGY=system`
environment already set, so `cargo build` works out of the box.
