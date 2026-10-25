#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run production storage tests without Linux TPM/vision dependencies.

Spectrum and key size are inert stand-ins. This does not simulate power loss.
"""
import json
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="gaze-storage-tests-") as tmp:
    crate = Path(tmp)
    (crate / "src").mkdir()
    (crate / "Cargo.toml").write_text('''[package]
name = "gaze-storage-tests"
version = "0.0.0"
edition = "2024"
[dependencies]
aes-gcm = "0.11.1"
anyhow = "1.0.104"
getrandom = "0.4.3"
libc = "0.2.191"
ndarray = "0.17.2"
uuid = { version = "1.29.0", features = ["v4"] }
tracing = "0.1.44"
tempfile = "=3.27.0"
gaze-core = { path = ''' + json.dumps(str(root / "crates/gaze-core")) + " }\n")
    library = '''#![allow(dead_code)]
extern crate self as gaze_vision;
extern crate self as gaze_security;
pub mod face {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Spectrum { Rgb, Ir }
}
pub mod tpm { pub const KEY_LEN: usize = 32; }
'''
    for name, source in [("crypto", "crates/gazed/src/crypto.rs"), ("users", "crates/gazed/src/users/mod.rs")]:
        library += f'#[path = {json.dumps(str(root / source))}]\nmod {name};\n'
    duress = (root / "crates/gazed/src/duress.rs").read_text()
    duress = duress[duress.index("pub struct DuressLockout"):duress.index("#[cfg(test)]")]
    library += 'use std::collections::HashSet;\nuse std::sync::Mutex;\nuse std::path::{Path, PathBuf};\nuse std::os::unix::fs::{OpenOptionsExt, PermissionsExt};\nuse users::UserDatabase;\n' + duress
    library += '''
#[test]
fn duress_create_clear_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let markers = dir.path().join("duress");
    let locks = DuressLockout::new(&markers);
    locks.lock("alice").unwrap();
    let restarted = DuressLockout::new(&markers);
    assert!(restarted.is_locked("alice"));
    assert!(restarted.clear("alice").unwrap());
    assert!(!DuressLockout::new(&markers).is_locked("alice"));
    assert!(!restarted.clear("alice").unwrap());
}
'''
    (crate / "src/lib.rs").write_text(library)
    subprocess.run(["cargo", "test", "--manifest-path", str(crate / "Cargo.toml")], check=True)
