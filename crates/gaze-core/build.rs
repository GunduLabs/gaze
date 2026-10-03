// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

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
    #[serde(default)]
    requires_ir_yuy2: bool,
}

#[derive(Deserialize)]
struct Emitter {
    // Simple format.
    unit: Option<u8>,
    selector: Option<u8>,
    control_bytes: Option<Vec<u8>>,
    off_control_bytes: Option<Vec<u8>>,
    // Multi-step format.
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
    requires_ir_yuy2: bool,
    on: Vec<ProcessedStep>,
    off: Vec<ProcessedStep>,
}

struct ProcessedStep {
    unit: u8,
    selector: u8,
    query: String,
    bytes: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct I2cProfileFile {
    device: I2cDevice,
    emitter: I2cEmitterSpec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct I2cDevice {
    name: String,
    source: String,
    capture_node: String,
    capture_name: String,
    source_marker: Option<String>,
    source_driver: Option<String>,
    sensor_device: String,
    sensor_driver: String,
    i2c_address: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct I2cEmitterSpec {
    register: u16,
    register_width: u8,
    mask: u8,
    on: u8,
    off: u8,
}

struct ProcessedI2cProfile {
    device: I2cDevice,
    emitter: I2cEmitterSpec,
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

    let i2c_profiles_dir = manifest_dir.join("i2c-ir-profiles");
    println!("cargo:rerun-if-changed={}", i2c_profiles_dir.display());
    let mut i2c_profiles = Vec::new();
    if i2c_profiles_dir.exists() {
        let mut files = fs::read_dir(&i2c_profiles_dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", i2c_profiles_dir.display()))
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("toml"))
            .collect::<Vec<_>>();
        files.sort();

        for path in files {
            println!("cargo:rerun-if-changed={}", path.display());
            i2c_profiles.push(parse_i2c_profile(&path));
        }

        let mut seen_nodes = std::collections::HashSet::new();
        for profile in &i2c_profiles {
            if !seen_nodes.insert(profile.device.capture_node.as_str()) {
                panic!(
                    "duplicate I2C IR profile for capture node {}",
                    profile.device.capture_node
                );
            }
        }
    }
    fs::write(
        out_dir.join("i2c_ir_profiles.rs"),
        render_i2c_profiles(&i2c_profiles),
    )
    .unwrap();
}

fn parse_i2c_profile(path: &Path) -> ProcessedI2cProfile {
    let stem = path.file_stem().unwrap().to_str().unwrap();
    assert!(
        !stem.is_empty()
            && stem
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "{}: I2C profile file names must use only lowercase letters, digits, and '-'",
        path.display()
    );
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let profile: I2cProfileFile = toml_edit::de::from_str(&text)
        .unwrap_or_else(|e| panic!("failed to parse TOML in {}: {e}", path.display()));

    let device = profile.device;
    let emitter = profile.emitter;
    for (field, value) in [
        ("device.name", &device.name),
        ("device.source", &device.source),
        ("device.capture_name", &device.capture_name),
        ("device.sensor_driver", &device.sensor_driver),
    ] {
        assert!(
            !value.trim().is_empty(),
            "{}: {field} is empty",
            path.display()
        );
    }
    assert!(
        device.capture_node.starts_with("/dev/video"),
        "{}: device.capture_node must be a /dev/video* path",
        path.display()
    );
    assert!(
        !device.sensor_device.is_empty()
            && !device.sensor_device.contains('/')
            && !device.sensor_device.starts_with('.'),
        "{}: device.sensor_device must be a bare /sys/bus/i2c/devices entry name",
        path.display()
    );
    assert!(
        (0x08..=0x77).contains(&device.i2c_address),
        "{}: device.i2c_address must be a non-reserved 7-bit address (0x08..=0x77)",
        path.display()
    );
    assert!(
        device.source_marker.is_some() == device.source_driver.is_some(),
        "{}: source_marker and source_driver must be specified together",
        path.display()
    );
    assert!(
        device
            .source_marker
            .as_deref()
            .is_none_or(|marker| marker.starts_with("/run/")),
        "{}: device.source_marker must live under /run",
        path.display()
    );
    assert!(
        matches!(emitter.register_width, 1 | 2),
        "{}: emitter.register_width must be 1 or 2",
        path.display()
    );
    assert!(
        emitter.register_width == 2 || emitter.register <= 0xff,
        "{}: emitter.register does not fit in one byte",
        path.display()
    );
    assert!(
        emitter.mask != 0,
        "{}: emitter.mask is zero",
        path.display()
    );
    assert!(
        emitter.on & !emitter.mask == 0 && emitter.off & !emitter.mask == 0,
        "{}: emitter.on and emitter.off must only set bits inside emitter.mask",
        path.display()
    );
    assert!(
        emitter.on != emitter.off,
        "{}: emitter.on and emitter.off are identical",
        path.display()
    );

    ProcessedI2cProfile { device, emitter }
}

fn render_i2c_profiles(profiles: &[ProcessedI2cProfile]) -> String {
    let mut out = String::new();
    writeln!(out, "// @generated by crates/gaze-core/build.rs").unwrap();
    writeln!(
        out,
        "// Do not edit directly; add i2c-ir-profiles/*.toml instead.\n"
    )
    .unwrap();

    writeln!(out, "pub const I2C_IR_PROFILES: &[I2cIrProfile] = &[").unwrap();
    for profile in profiles {
        let device = &profile.device;
        let emitter = &profile.emitter;
        writeln!(out, "    I2cIrProfile {{").unwrap();
        writeln!(out, "        name: {:?},", device.name).unwrap();
        writeln!(out, "        source: {:?},", device.source).unwrap();
        writeln!(out, "        capture_node: {:?},", device.capture_node).unwrap();
        writeln!(out, "        capture_name: {:?},", device.capture_name).unwrap();
        render_optional_string(&mut out, "source_marker", device.source_marker.as_deref());
        render_optional_string(&mut out, "source_driver", device.source_driver.as_deref());
        writeln!(out, "        sensor_device: {:?},", device.sensor_device).unwrap();
        writeln!(out, "        sensor_driver: {:?},", device.sensor_driver).unwrap();
        writeln!(out, "        address: 0x{:02x},", device.i2c_address).unwrap();
        writeln!(out, "        register: 0x{:04x},", emitter.register).unwrap();
        writeln!(out, "        register_width: {},", emitter.register_width).unwrap();
        writeln!(out, "        mask: 0x{:02x},", emitter.mask).unwrap();
        writeln!(out, "        on: 0x{:02x},", emitter.on).unwrap();
        writeln!(out, "        off: 0x{:02x},", emitter.off).unwrap();
        writeln!(out, "    }},").unwrap();
    }
    writeln!(out, "];\n").unwrap();
    out
}

fn render_optional_string(out: &mut String, field: &str, value: Option<&str>) {
    match value {
        Some(value) => writeln!(out, "        {field}: Some({value:?}),").unwrap(),
        None => writeln!(out, "        {field}: None,").unwrap(),
    }
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
    let p: ProfileFile = toml_edit::de::from_str(&text)
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
        requires_ir_yuy2: p.device.requires_ir_yuy2,
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
    writeln!(out, "// @generated by crates/gaze-core/build.rs").unwrap();
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
        writeln!(
            out,
            "        requires_ir_yuy2: {},",
            profile.requires_ir_yuy2
        )
        .unwrap();
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
