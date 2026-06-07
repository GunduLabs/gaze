# IR emitter device profiles

Each `*.toml` file maps a USB UVC camera VID:PID to the raw UVC extension-unit control sequence needed to enable and disable its IR emitter.

File names are lowercase hex without `0x`: `vvvv-pppp.toml`.

## Simple single-control format

```toml
[device]
vendor_id  = 0x04F2
product_id = 0xB6D9
name       = "Example IR camera"
source     = "URL or issue where the bytes were confirmed"

[emitter]
unit              = 14
selector          = 6
control_bytes     = [1, 3, 2, 0, 0, 0, 0, 0, 0]
off_control_bytes = [1, 3, 1, 0, 0, 0, 0, 0, 0]
```

If `off_control_bytes` is omitted, zeros of the same length are used.

## Multi-step sequence format

Use this for devices that require multiple UVC requests or `GET_CUR` priming steps:

```toml
[device]
vendor_id  = 0x0BDA
product_id = 0x5767
name       = "Realtek/Dell 0JCXG0 Integrated_Webcam_HD"
source     = "https://github.com/SeeleVolleri/research_0jcxg0"

[[emitter.on]]
unit = 4
selector = 10
query = "set_cur"
control_bytes = [0xff, 0, 0, 0, 0, 0, 0, 0]

[[emitter.on]]
unit = 4
selector = 11
query = "get_cur"
size = 8
```

Supported `query` values are `set_cur` and `get_cur`.

Gaze also has a safe runtime fallback for Microsoft Face Authentication XU controls: when no VID:PID profile exists, it probes read-only `GET_CUR` on selector `0x06` for the standard 9-byte `[1, 3, mode, 0, ...]` shape and uses `[1,3,2,0,...]`/`[1,3,1,0,...]` if found.
