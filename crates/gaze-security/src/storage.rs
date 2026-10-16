// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Best-effort offline-attack warning, not an encryption or boot-integrity attestation.
//! Follow the device which actually holds the state directory, not just the root mount.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskEncryption {
    Encrypted,
    Unencrypted,
    Unknown,
}

pub fn disk_encryption(path: &Path) -> DiskEncryption {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let mut metadata = None;
        for ancestor in path.ancestors() {
            match std::fs::metadata(ancestor) {
                Ok(meta) => {
                    metadata = Some(meta);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return DiskEncryption::Unknown,
            }
        }
        let Some(meta) = metadata else {
            return DiskEncryption::Unknown;
        };
        let device = Path::new("/sys/dev/block").join(format!(
            "{}:{}",
            libc::major(meta.dev()),
            libc::minor(meta.dev())
        ));
        device_encryption(&device, &mut std::collections::HashSet::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        DiskEncryption::Unknown
    }
}

/// A separately mounted template, key, or credential directory must not be hidden
/// by an encrypted root filesystem. Unknown on any path remains unknown overall.
pub fn disk_encryption_for_paths(paths: &[&Path]) -> DiskEncryption {
    if paths.is_empty() {
        return DiskEncryption::Unknown;
    }
    paths.iter().map(|path| disk_encryption(path)).fold(
        DiskEncryption::Encrypted,
        |combined, next| match (combined, next) {
            (DiskEncryption::Unencrypted, _) | (_, DiskEncryption::Unencrypted) => {
                DiskEncryption::Unencrypted
            }
            (DiskEncryption::Unknown, _) | (_, DiskEncryption::Unknown) => DiskEncryption::Unknown,
            _ => DiskEncryption::Encrypted,
        },
    )
}

/// Unknown must produce a warning too: network, overlay, btrfs multi-device, and
/// inaccessible sysfs cannot safely be called either encrypted or unencrypted.
pub fn encryption_warning(status: DiskEncryption) -> Option<&'static str> {
    match status {
        DiskEncryption::Encrypted => None,
        DiskEncryption::Unencrypted => Some(
            "Gaze state storage appears unencrypted. TPM sealing has no PCR policy: someone who boots another OS on this machine can unseal accessible keys and recover face templates and enrolled keyring/wallet passwords. Enable full-disk encryption for the storage holding /var/lib/gaze; TPM encryption alone does not prevent this attack.",
        ),
        DiskEncryption::Unknown => Some(
            "Gaze could not verify encryption of its state storage. TPM sealing has no PCR policy: a live-USB/root attacker on this machine can unseal accessible keys, including enrolled keyring/wallet passwords. Verify that the storage holding /var/lib/gaze is protected by full-disk encryption; do not assume the TPM protects against an alternate boot.",
        ),
    }
}

#[cfg(any(target_os = "linux", test))]
fn device_encryption(
    path: &Path,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
) -> DiskEncryption {
    let Ok(path) = std::fs::canonicalize(path) else {
        return DiskEncryption::Unknown;
    };
    if !visited.insert(path.clone()) {
        return DiskEncryption::Unknown;
    }
    let result = (|| {
        if path.join("dm").exists() {
            let Ok(uuid) = std::fs::read_to_string(path.join("dm/uuid")) else {
                return DiskEncryption::Unknown;
            };
            // cryptsetup also manages VERITY and INTEGRITY mappings: those do not
            // encrypt anything. Only recognize actual encryption target UUIDs.
            if [
                "CRYPT-LUKS1-",
                "CRYPT-LUKS2-",
                "CRYPT-PLAIN-",
                "CRYPT-TCRYPT-",
                "CRYPT-BITLK-",
                "CRYPT-FVAULT2-",
            ]
            .iter()
            .any(|prefix| uuid.starts_with(prefix))
            {
                return DiskEncryption::Encrypted;
            }
        }
        if path.join("partition").exists() {
            return path.parent().map_or(DiskEncryption::Unknown, |parent| {
                device_encryption(parent, visited)
            });
        }
        let Ok(slaves) = std::fs::read_dir(path.join("slaves")) else {
            return DiskEncryption::Unknown;
        };
        let mut statuses = Vec::new();
        for slave in slaves {
            let Ok(slave) = slave else {
                return DiskEncryption::Unknown;
            };
            statuses.push(device_encryption(&slave.path(), visited));
        }
        if !statuses.is_empty() {
            if statuses.contains(&DiskEncryption::Unencrypted) {
                return DiskEncryption::Unencrypted;
            }
            if statuses.contains(&DiskEncryption::Unknown) {
                return DiskEncryption::Unknown;
            }
            return DiskEncryption::Encrypted;
        }
        if path.join("dm").exists() || path.join("loop").exists() {
            DiskEncryption::Unknown
        } else {
            DiskEncryption::Unencrypted
        }
    })();
    visited.remove(&path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn every_backing_path_must_be_encrypted() {
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("disk");
        let crypt = root.path().join("crypt");
        let lvm = root.path().join("lvm");
        for path in [&plain, &crypt, &lvm] {
            fs::create_dir_all(path.join("slaves")).unwrap();
        }
        fs::create_dir(crypt.join("dm")).unwrap();
        fs::write(crypt.join("dm/uuid"), "CRYPT-LUKS2-test").unwrap();
        fs::create_dir(lvm.join("dm")).unwrap();
        fs::write(lvm.join("dm/uuid"), "LVM-test").unwrap();
        let detect = |path: &Path| device_encryption(path, &mut Default::default());
        assert_eq!(detect(&plain), DiskEncryption::Unencrypted);
        assert_eq!(detect(&crypt), DiskEncryption::Encrypted);
        symlink(&crypt, lvm.join("slaves/crypt")).unwrap();
        assert_eq!(detect(&lvm), DiskEncryption::Encrypted);
        symlink(&plain, lvm.join("slaves/plain")).unwrap();
        assert_eq!(detect(&lvm), DiskEncryption::Unencrypted);
        fs::remove_file(lvm.join("slaves/plain")).unwrap();
        symlink(&lvm, lvm.join("slaves/cycle")).unwrap();
        assert_eq!(detect(&lvm), DiskEncryption::Unknown);
        assert_eq!(
            detect(&root.path().join("missing")),
            DiskEncryption::Unknown
        );
    }

    #[test]
    fn integrity_only_mappings_are_not_reported_as_encrypted() {
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("disk");
        let mapping = root.path().join("mapping");
        std::fs::create_dir_all(plain.join("slaves")).unwrap();
        std::fs::create_dir_all(mapping.join("slaves")).unwrap();
        std::fs::create_dir(mapping.join("dm")).unwrap();
        symlink(&plain, mapping.join("slaves/disk")).unwrap();
        for uuid in ["CRYPT-VERITY-test", "CRYPT-INTEGRITY-test"] {
            std::fs::write(mapping.join("dm/uuid"), uuid).unwrap();
            assert_eq!(
                device_encryption(&mapping, &mut Default::default()),
                DiskEncryption::Unencrypted
            );
        }
    }

    #[test]
    fn unknown_storage_is_not_a_security_checkmark() {
        assert!(encryption_warning(DiskEncryption::Encrypted).is_none());
        for state in [DiskEncryption::Unencrypted, DiskEncryption::Unknown] {
            let warning = encryption_warning(state).unwrap();
            assert!(warning.contains("no PCR policy"));
            assert!(warning.contains("passwords"));
        }
    }
}
