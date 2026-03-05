# CLI Reference

The `gaze` CLI communicates with the running `gazed` daemon over DBus. All commands accept `-u, --user <USER>` to target a specific user instead of `$USER`.

## Commands

### `gaze auth`

Authenticate the current user via webcam.

```bash
gaze auth [OPTIONS]
```

| Option | Description |
|---|---|
| `-u, --user <USER>` | Authenticate as a specific user (default: `$USER`) |
| `--perf` | Print step-by-step timing metrics (camera init, detection, match) |
| `-v, --verbose` | Show a table of all enrolled faces with their similarity scores and whether they passed the threshold |

The command opens the camera and waits until a face is detected and centered. While scanning, a spinner shows real-time status (`No face detected`, `Face is clipped`, etc.). Once a valid frame is captured it is sent to the daemon for matching.

**Results:**
- **Green ✓** — `✓ Authenticated as: <face> (<pct>%, <ms>ms)`
- **Red ✗** — `✗ Access Denied. (<ms>ms)`

---

### `gaze add-face <NAME>`

Enroll a new face with a guided multi-angle capture session.

```bash
gaze add-face <NAME> [OPTIONS]
```

| Argument/Option | Description |
|---|---|
| `<NAME>` | Name to assign to this face (e.g. `default`, `glasses`) |
| `-u, --user <USER>` | Enroll for a specific user (default: `$USER`) |

The capture session walks you through multiple angle prompts. Capture is automatic when the face is centered and stable — no button press needed. The more angles captured, the more robust recognition will be.

---

### `gaze refine-face <NAME>`

Add additional captures to an existing enrolled face to improve recognition accuracy.

```bash
gaze refine-face <NAME> [OPTIONS]
```

| Argument/Option | Description |
|---|---|
| `<NAME>` | Name of the face to refine |
| `-u, --user <USER>` | Target a specific user (default: `$USER`) |

Use this if recognition is failing under certain lighting conditions or angles — it adds new embeddings to the existing face without replacing what's already there.

---

### `gaze rename-face <FROM> <TO>`

Rename an enrolled face.

```bash
gaze rename-face <FROM> <TO> [OPTIONS]
```

| Argument/Option | Description |
|---|---|
| `<FROM>` | Current name of the face |
| `<TO>` | New name to assign |
| `-u, --user <USER>` | Target a specific user (default: `$USER`) |

---

### `gaze list-faces`

List all enrolled faces for a user, along with the number of captures stored for each.

```bash
gaze list-faces [OPTIONS]
```

| Option | Description |
|---|---|
| `-u, --user <USER>` | List faces for a specific user (default: `$USER`) |

---

### `gaze remove-face <NAME>`

Remove a specific enrolled face and all its stored captures.

```bash
gaze remove-face <NAME> [OPTIONS]
```

| Argument/Option | Description |
|---|---|
| `<NAME>` | Name of the face to remove |
| `-u, --user <USER>` | Target a specific user (default: `$USER`) |

---

### `gaze clear-user`

Remove all enrolled faces and data for a user. This is destructive and cannot be undone.

```bash
gaze clear-user [OPTIONS]
```

| Option | Description |
|---|---|
| `-u, --user <USER>` | Target a specific user (default: `$USER`) |
