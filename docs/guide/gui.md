# GUI

Launch the GTK4/Adwaita enrollment and authentication interface:

```bash
gaze-gui
```

The GUI provides the same face enrollment and authentication features as the CLI, with a graphical interface:

- **Enroll a new face** — guided multi-angle capture session
- **Test authentication** — shows a color-coded result (green/red/yellow) using the same scheme as the CLI
- **Manage faces** — list and remove enrolled faces

The GUI communicates with the running `gazed` daemon over DBus, just like the CLI.
