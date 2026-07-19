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
      # Optional, saves a second nixpkgs evaluation:
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

            # Attempt face auth (password stays as fallback) for these
            # PAM services:
            pam.services = {
              sudo = { };
              polkit-1 = { };
            };
          };
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
| `services.gaze.pam.services` | `{ }` | PAM services to insert face auth into, e.g. `{ sudo = { }; hyprlock.simultaneous = true; }` |
| `services.gaze.pam.services.<name>.control` | `"sufficient"` | PAM control field for the rule |
| `services.gaze.pam.services.<name>.simultaneous` | `false` | Use `pam_gaze_grosshack.so` (face and password prompt at the same time) instead of sequential `pam_gaze.so` |
| `services.gaze.gui.enable` | `false` | Install `gaze-gui` |
| `services.gaze.gnome.enable` | `false` | Install the GNOME Shell extension and the `gdm-face` PAM service |
| `services.gaze.gnome.gdmFaceLogin` | `false` | Also enable face auth at the GDM login screen (read the [GNOME guide](/guide/gnome) first) |

The gaze PAM rule is inserted just before `pam_unix`, so your password always
remains a fallback. For anything the options don't cover, use
`security.pam.services.<name>.rules` directly with
`${config.services.gaze.package}/lib/security/pam_gaze.so`.

### GNOME lock screen

```nix
services.gaze.gnome.enable = true;
```

Then, from your GNOME session, enable the extension for your user:

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

Log out and back in once if the lock screen does not pick it up immediately.
GDM *login* face auth stays disabled unless you set
`services.gaze.gnome.gdmFaceLogin = true;`. The GUI's GDM toggle cannot work
on NixOS, because the GDM dconf database is managed by your NixOS
configuration.

### hyprlock

```nix
programs.hyprlock.enable = true;
services.gaze.pam.services.hyprlock = { };
# or, for simultaneous face + password:
# services.gaze.pam.services.hyprlock.simultaneous = true;
```

This modifies the `hyprlock` PAM service in place, so no `auth_pam_module`
changes in `hyprlock.conf` are needed. See the [Hyprland guide](/guide/hyprland)
for behavior details.

### KDE Plasma and other desktops

Add the relevant PAM service names, e.g.:

```nix
services.gaze.pam.services = {
  kde = { };          # Plasma lock screen
  login = { };        # console/display-manager login
};
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
