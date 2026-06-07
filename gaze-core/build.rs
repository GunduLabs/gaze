use serde::Deserialize;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct ProfileFile {
    device: Device,
    emitter: Emitter,
}

#[derive(Deserialize)]
struct Device {
    vendor_id: u16,
    product_id: u16,
    name: String,
    source: Option<String>,
}

#[derive(Deserialize)]
struct Emitter {
    // Simple format
    unit: Option<u8>,
    selector: Option<u8>,
    control_bytes: Option<Vec<u8>>,
    off_control_bytes: Option<Vec<u8>>,
    // Multi-step format
    on: Option<Vec<Step>>,
    off: Option<Vec<Step>>,
}

#[derive(Deserialize)]
struct Step {
    unit: u8,
    selector: u8,
    #[serde(default = "default_query")]
    query: String,
    control_bytes: Option<Vec<u8>>,
    payload: Option<Vec<u8>>,
    size: Option<usize>,
}

fn default_query() -> String {
    "set_cur".to_string()
}

struct ProcessedProfile {
    ident: String,
    vid: u16,
    pid: u16,
    name: String,
    source: String,
    on: Vec<ProcessedStep>,
    off: Vec<ProcessedStep>,
}

struct ProcessedStep {
    unit: u8,
    selector: u8,
    query: String,
    bytes: Vec<u8>,
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let profiles_dir = manifest_dir.join("ir-profiles");
    println!("cargo:rerun-if-changed={}", profiles_dir.display());

    let mut profiles = Vec::new();
    if profiles_dir.exists() {
        let mut files = fs::read_dir(&profiles_dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", profiles_dir.display()))
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("toml"))
            .collect::<Vec<_>>();
        files.sort();

        for path in files {
            println!("cargo:rerun-if-changed={}", path.display());
            profiles.push(parse_profile(&path));
        }

        let mut seen = std::collections::HashSet::new();
        for profile in &profiles {
            if !seen.insert((profile.vid, profile.pid)) {
                panic!(
                    "duplicate IR profile for {:04x}:{:04x}",
                    profile.vid, profile.pid
                );
            }
        }
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("ir_devices.rs"), render(&profiles)).unwrap();
}

fn parse_profile(path: &Path) -> ProcessedProfile {
    let file_stem = path.file_stem().unwrap().to_str().unwrap();
    let (file_vid, file_pid) = parse_filename_ids(file_stem).unwrap_or_else(|| {
        panic!(
            "profile file must be named vvvv-pppp.toml: {}",
            path.display()
        )
    });

    let text = fs::read_to_string(path).unwrap();
    let p: ProfileFile = toml::from_str(&text)
        .unwrap_or_else(|e| panic!("failed to parse TOML in {}: {e}", path.display()));

    if (p.device.vendor_id, p.device.product_id) != (file_vid, file_pid) {
        panic!(
            "{} has device {:04x}:{:04x}, but file name is {:04x}:{:04x}",
            path.display(),
            p.device.vendor_id,
            p.device.product_id,
            file_vid,
            file_pid
        );
    }

    let (on, off) = if let Some(on_steps) = p.emitter.on {
        let on = on_steps
            .into_iter()
            .map(|s| process_step(s, path))
            .collect();
        let off = p
            .emitter
            .off
            .unwrap_or_else(|| panic!("{} uses emitter.on but has no emitter.off", path.display()))
            .into_iter()
            .map(|s| process_step(s, path))
            .collect();
        (on, off)
    } else {
        let unit = p
            .emitter
            .unit
            .unwrap_or_else(|| panic!("{} missing emitter.unit", path.display()));
        let selector = p
            .emitter
            .selector
            .unwrap_or_else(|| panic!("{} missing emitter.selector", path.display()));
        let control_bytes = p
            .emitter
            .control_bytes
            .unwrap_or_else(|| panic!("{} missing emitter.control_bytes", path.display()));
        let off_bytes = p
            .emitter
            .off_control_bytes
            .unwrap_or_else(|| vec![0; control_bytes.len()]);

        (
            vec![ProcessedStep {
                unit,
                selector,
                query: "set_cur".to_string(),
                bytes: control_bytes,
            }],
            vec![ProcessedStep {
                unit,
                selector,
                query: "set_cur".to_string(),
                bytes: off_bytes,
            }],
        )
    };

    ProcessedProfile {
        ident: format!(
            "DEVICE_{}",
            file_stem.replace('-', "_").to_ascii_uppercase()
        ),
        vid: p.device.vendor_id,
        pid: p.device.product_id,
        name: p.device.name,
        source: p
            .device
            .source
            .unwrap_or_else(|| "gaze-core profile".into()),
        on,
        off,
    }
}

fn process_step(step: Step, path: &Path) -> ProcessedStep {
    let bytes = step
        .control_bytes
        .or(step.payload)
        .or_else(|| step.size.map(|s| vec![0; s]))
        .unwrap_or_else(|| {
            panic!(
                "{}: step missing control_bytes, payload, or size",
                path.display()
            )
        });

    ProcessedStep {
        unit: step.unit,
        selector: step.selector,
        query: step.query.to_lowercase(),
        bytes,
    }
}

fn render(profiles: &[ProcessedProfile]) -> String {
    let mut out = String::new();
    writeln!(out, "// @generated by gaze-core/build.rs").unwrap();
    writeln!(
        out,
        "// Do not edit directly; add ir-profiles/*.toml instead.\n"
    )
    .unwrap();

    for profile in profiles {
        render_steps(&mut out, &profile.ident, "ON", &profile.on);
        render_steps(&mut out, &profile.ident, "OFF", &profile.off);
    }

    writeln!(out, "pub const IR_DEVICES: &[IrDevice] = &[").unwrap();
    for profile in profiles {
        writeln!(out, "    IrDevice {{").unwrap();
        writeln!(out, "        vid: 0x{:04x},", profile.vid).unwrap();
        writeln!(out, "        pid: 0x{:04x},", profile.pid).unwrap();
        writeln!(out, "        name: {:?},", profile.name).unwrap();
        writeln!(out, "        on_sequence: {}_ON,", profile.ident).unwrap();
        writeln!(out, "        off_sequence: {}_OFF,", profile.ident).unwrap();
        writeln!(out, "        source: {:?},", profile.source).unwrap();
        writeln!(out, "    }},").unwrap();
    }
    writeln!(out, "];\n").unwrap();
    out
}

fn render_steps(out: &mut String, ident: &str, kind: &str, steps: &[ProcessedStep]) {
    for (idx, step) in steps.iter().enumerate() {
        write!(out, "const {ident}_{kind}_{idx}_BYTES: &[u8] = &[").unwrap();
        for (b_idx, byte) in step.bytes.iter().enumerate() {
            if b_idx != 0 {
                write!(out, ", ").unwrap();
            }
            write!(out, "0x{byte:02x}").unwrap();
        }
        writeln!(out, "];").unwrap();
    }
    writeln!(out, "const {ident}_{kind}: &[IrControl] = &[").unwrap();
    for (idx, step) in steps.iter().enumerate() {
        let query = match step.query.as_str() {
            "get_cur" | "get" => "IrQuery::GetCur",
            _ => "IrQuery::SetCur",
        };
        writeln!(
            out,
            "    IrControl {{ unit: 0x{:02x}, selector: 0x{:02x}, query: {query}, payload: {ident}_{kind}_{idx}_BYTES }},",
            step.unit,
            step.selector
        )
        .unwrap();
    }
    writeln!(out, "];\n").unwrap();
}

fn parse_filename_ids(file_stem: &str) -> Option<(u16, u16)> {
    let (vid, pid) = file_stem.split_once('-')?;
    if vid.len() != 4 || pid.len() != 4 {
        return None;
    }
    Some((
        u16::from_str_radix(vid, 16).ok()?,
        u16::from_str_radix(pid, 16).ok()?,
    ))
}
