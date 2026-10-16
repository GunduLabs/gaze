// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sealed-blob persistence, deliberately independent of TPM hardware and ESAPI.

use anyhow::{Context as _, anyhow, ensure};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub(super) const PUB_FILE: &str = "dek.pub";
pub(super) const PRIV_FILE: &str = "dek.priv";
pub(super) const BUNDLE_FILE: &str = "dek.blob";
const BUNDLE_MAGIC: &[u8; 5] = b"GZDK1";

fn read_private_file(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o077 == 0
            && meta.nlink() == 1,
        "unsafe sealed-key file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(16385).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 16384, "sealed-key file is too large");
    Ok(Some(bytes))
}

pub(super) fn read_sealed_blobs(dir: &Path) -> anyhow::Result<Option<(Vec<u8>, Vec<u8>)>> {
    if let Some(bytes) = read_private_file(&dir.join(BUNDLE_FILE))? {
        ensure!(
            bytes.len() >= 9 && bytes.starts_with(BUNDLE_MAGIC),
            "invalid sealed-key bundle"
        );
        let public_len = u32::from_le_bytes(bytes[5..9].try_into()?) as usize;
        let private_len = bytes.len().saturating_sub(9 + public_len);
        ensure!(
            (1..=4096).contains(&public_len) && (1..=4096).contains(&private_len),
            "invalid sealed-key bundle lengths"
        );
        return Ok(Some((
            bytes[9..9 + public_len].to_vec(),
            bytes[9 + public_len..].to_vec(),
        )));
    }
    match (
        read_private_file(&dir.join(PUB_FILE))?,
        read_private_file(&dir.join(PRIV_FILE))?,
    ) {
        (None, None) => Ok(None),
        (Some(public), Some(private)) => Ok(Some((public, private))),
        _ => Err(anyhow!(
            "incomplete legacy TPM key pair; restore the missing blob from backup; refusing to generate a replacement key"
        )),
    }
}

pub(super) fn persist_bundle(dir: &Path, public: &[u8], private: &[u8]) -> anyhow::Result<()> {
    ensure!(
        (1..=4096).contains(&public.len()) && (1..=4096).contains(&private.len()),
        "invalid sealed-key lengths"
    );
    let mut bytes = BUNDLE_MAGIC.to_vec();
    bytes.extend_from_slice(&(public.len() as u32).to_le_bytes());
    bytes.extend_from_slice(public);
    bytes.extend_from_slice(private);
    match read_private_file(&dir.join(BUNDLE_FILE))? {
        Some(existing) => ensure!(
            existing == bytes,
            "sealed-key bundle changed; refusing to overwrite it"
        ),
        None => write_private_file(&dir.join(BUNDLE_FILE), &bytes)?,
    }
    // Sync even an existing bundle before removing either legacy blob: it may have
    // survived a previous rename whose directory sync failed.
    File::open(dir)?.sync_all()?;
    for name in [PUB_FILE, PRIV_FILE] {
        match fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    File::open(dir)?.sync_all()?;
    Ok(())
}

pub(super) fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a private directory",
            path.display()
        )));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(super) fn write_private_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("sealed key path has no parent directory")?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("dek"),
        uuid::Uuid::new_v4()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("failed to create {}", tmp.display()))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&tmp);
        return Err(error).with_context(|| format!("failed to write {}", tmp.display()));
    }
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn bundle_round_trip_and_interrupted_legacy_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        ensure_private_dir(dir.path()).unwrap();
        write_private_file(&dir.path().join(PUB_FILE), b"public").unwrap();
        assert!(read_sealed_blobs(dir.path()).is_err());
        write_private_file(&dir.path().join(PRIV_FILE), b"private").unwrap();
        persist_bundle(dir.path(), b"public", b"private").unwrap();
        // A crash may leave either legacy file behind; the durable bundle is authoritative.
        write_private_file(&dir.path().join(PRIV_FILE), b"old private").unwrap();
        assert_eq!(
            read_sealed_blobs(dir.path()).unwrap(),
            Some((b"public".to_vec(), b"private".to_vec()))
        );
        persist_bundle(dir.path(), b"public", b"private").unwrap();
        assert!(!dir.path().join(PRIV_FILE).exists());
        assert_eq!(
            fs::metadata(dir.path().join(BUNDLE_FILE)).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn corrupt_or_replaced_bundles_never_fall_back_to_legacy_keys() {
        let dir = tempfile::tempdir().unwrap();
        write_private_file(&dir.path().join(PUB_FILE), b"public").unwrap();
        write_private_file(&dir.path().join(PRIV_FILE), b"private").unwrap();
        for bytes in [
            b"GZDK1".as_slice(),
            b"GZDK1\xff\xff\xff\xff",
            b"GZDK1\0\0\0\0private",
        ] {
            write_private_file(&dir.path().join(BUNDLE_FILE), bytes).unwrap();
            assert!(read_sealed_blobs(dir.path()).is_err());
            assert!(persist_bundle(dir.path(), b"public", b"private").is_err());
            assert!(dir.path().join(PUB_FILE).exists());
            assert!(dir.path().join(PRIV_FILE).exists());
        }
    }

    #[test]
    fn unsafe_blob_types_and_permissions_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BUNDLE_FILE);
        let target = dir.path().join("target");
        write_private_file(&target, b"bundle").unwrap();
        symlink(&target, &path).unwrap();
        assert!(read_sealed_blobs(dir.path()).is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&target, &path).unwrap();
        assert!(read_sealed_blobs(dir.path()).is_err());
        fs::remove_file(&path).unwrap();
        write_private_file(&path, b"bundle").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_sealed_blobs(dir.path()).is_err());
    }
}
