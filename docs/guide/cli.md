# CLI Reference

The `gaze` CLI communicates with the running `gazed` daemon over DBus.

## Commands

### `gaze auth`

Authenticate the current user via webcam.

```bash
gaze auth [OPTIONS]
```

| Option | Description |
|---|---|
| `-u, --user <USER>` | Authenticate as a specific user (default: `$USER`) |
| `--perf` | Print detailed step-by-step performance metrics |
| `-v, --verbose` | Show per-face similarity scores and match details |

**Result codes:**
- **Green ✓** — authenticated successfully
- **Red ✗** — access denied (face detected but not recognized)
- **Yellow !** — could not detect a face

---

### `gaze add-face <NAME>`

Enroll a new face with a guided multi-angle capture session.

```bash
gaze add-face <NAME> [OPTIONS]
```

| Argument/Option | Description |
|---|---|
| `<NAME>` | Name to assign to this face |
| `-u, --user <USER>` | Enroll for a specific user (default: `$USER`) |

The capture session prompts you to position your face at different angles. Capture is automatic when the face is centered and stable.

---

### `gaze refine-face <NAME>`

Add additional captures to improve recognition of an existing face.

```bash
gaze refine-face <NAME> [OPTIONS]
```

| Argument/Option | Description |
|---|---|
| `<NAME>` | Name of the face to refine |
| `-u, --user <USER>` | Target a specific user (default: `$USER`) |

---

### `gaze list-faces`

List all enrolled faces for a user.

```bash
gaze list-faces [OPTIONS]
```

| Option | Description |
|---|---|
| `-u, --user <USER>` | List faces for a specific user (default: `$USER`) |

---

### `gaze remove-face <NAME>`

Remove a specific enrolled face.

```bash
gaze remove-face <NAME> [OPTIONS]
```

| Argument/Option | Description |
|---|---|
| `<NAME>` | Name of the face to remove |
| `-u, --user <USER>` | Target a specific user (default: `$USER`) |

---

### `gaze clear-user`

Remove all enrolled faces and data for a user.

```bash
gaze clear-user [OPTIONS]
```

| Option | Description |
|---|---|
| `-u, --user <USER>` | Target a specific user (default: `$USER`) |
