// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Root-only, per-account GNOME Keyring credentials. No IPC or biometric verdicts live here:
//! the PAM caller must finish authentication before calling `load`.

use aes_gcm::aead::{AeadInOut, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, ensure};
use sha2::{Digest, Sha256};
use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
pub use zeroize::Zeroizing;

pub const STORE_DIR: &str = "/var/lib/gaze/keyring";
const MAGIC: &[u8; 4] = b"GZK1";
const MAX_BLOB: usize = 16384;
pub const MAX_PASSWORD: usize = 4096;
pub type Secret = Zeroizing<Vec<u8>>;

/// Not Debug/Clone: neither the password nor the shadow record belongs in diagnostics.
pub struct Account {
    uid: u32,
    binding: [u8; 32],
}

impl Account {
    fn uid(username: &str) -> anyhow::Result<u32> {
        ensure!(
            unsafe { libc::geteuid() } == 0,
            "keyring setup requires root"
        );
        let name = CString::new(username)?;
        let mut buffer = Zeroizing::new(vec![0u8; 65536]);
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                pwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        ensure!(status == 0 && !result.is_null(), "account not found");
        let uid = unsafe { (*result).pw_uid };
        ensure!(uid != 0, "root keyring enrollment is not supported");
        Ok(uid)
    }

    pub fn lookup(username: &str) -> anyhow::Result<Self> {
        let uid = Self::uid(username)?;
        let name = CString::new(username)?;
        let mut buffer = Zeroizing::new(vec![0u8; 65536]);

        let mut shadow = std::mem::MaybeUninit::<libc::spwd>::uninit();
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getspnam_r(
                name.as_ptr(),
                shadow.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        ensure!(
            status == 0 && !result.is_null(),
            "a readable shadow password record is required"
        );
        let shadow = unsafe { &*result };
        ensure!(
            !shadow.sp_pwdp.is_null() && shadow.sp_lstchg != 0,
            "account requires a password change"
        );
        let hash = unsafe { CStr::from_ptr(shadow.sp_pwdp) }.to_bytes();
        ensure!(
            !hash.is_empty() && !matches!(hash[0], b'!' | b'*'),
            "account password is empty or locked"
        );
        Ok(Self::bound_to(uid, username, hash))
    }

    fn bound_to(uid: u32, username: &str, hash: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"gaze-gnome-keyring-v1\0");
        digest.update(uid.to_le_bytes());
        digest.update(username.as_bytes());
        digest.update([0]);
        digest.update(hash);
        Self {
            uid,
            binding: digest.finalize().into(),
        }
    }
}

pub fn validate_password(password: &[u8]) -> anyhow::Result<()> {
    ensure!(
        !password.is_empty() && password.len() <= MAX_PASSWORD && !password.contains(&0),
        "keyring password must contain 1..=4096 bytes and no NUL"
    );
    Ok(())
}

// Keep decryption in a wiping buffer even when authentication of the ciphertext fails.
fn encrypt(
    key: &[u8; 32],
    account: &Account,
    password: &[u8],
    public: &[u8],
    private: &[u8],
) -> anyhow::Result<Vec<u8>> {
    validate_password(password)?;
    ensure!(
        public.len() <= 4096 && private.len() <= 4096,
        "invalid sealed key"
    );
    let mut nonce = [0; 12];
    getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!("random nonce unavailable"))?;
    let mut secret = Zeroizing::new(Vec::with_capacity(password.len() + 17));
    secret.extend_from_slice(password);
    secret.push(0); // pam_set_item copies this C string; never make an unwiped CString.
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("invalid key"))?;
    let tag = cipher
        .encrypt_inout_detached(
            &Nonce::from(nonce),
            &account.binding,
            secret.as_mut_slice().into(),
        )
        .map_err(|_| anyhow::anyhow!("credential encryption failed"))?;
    let mut blob = Vec::new();
    blob.extend_from_slice(MAGIC);
    blob.extend_from_slice(&(public.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(private.len() as u32).to_le_bytes());
    blob.extend_from_slice(public);
    blob.extend_from_slice(private);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&secret);
    blob.extend_from_slice(&tag);
    Ok(blob)
}

fn decrypt_with<F>(blob: &[u8], account: &Account, unseal: F) -> anyhow::Result<Secret>
where
    F: FnOnce(&[u8], &[u8]) -> anyhow::Result<[u8; 32]>,
{
    ensure!(
        blob.len() >= 12 && blob.len() <= MAX_BLOB && &blob[..4] == MAGIC,
        "invalid credential record"
    );
    let public_len = u32::from_le_bytes(blob[4..8].try_into()?) as usize;
    let private_len = u32::from_le_bytes(blob[8..12].try_into()?) as usize;
    ensure!(
        public_len <= 4096 && private_len <= 4096,
        "invalid sealed key length"
    );
    let end = 12 + public_len + private_len;
    ensure!(blob.len() > end + 12 + 16, "truncated credential record");
    let key = Zeroizing::new(unseal(
        &blob[12..12 + public_len],
        &blob[12 + public_len..end],
    )?);
    let cipher =
        Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| anyhow::anyhow!("invalid key"))?;
    let nonce =
        Nonce::try_from(&blob[end..end + 12]).map_err(|_| anyhow::anyhow!("invalid nonce"))?;
    let tag = aes_gcm::Tag::try_from(&blob[blob.len() - 16..])
        .map_err(|_| anyhow::anyhow!("invalid tag"))?;
    let mut password = Zeroizing::new(blob[end + 12..blob.len() - 16].to_vec());
    cipher
        .decrypt_inout_detached(
            &nonce,
            &account.binding,
            password.as_mut_slice().into(),
            &tag,
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "credential unavailable: account password changed, wrong TPM, or corrupt record"
            )
        })?;
    ensure!(password.last() == Some(&0), "invalid credential terminator");
    validate_password(&password[..password.len() - 1])?;
    Ok(password)
}

/// Verify every path component before accessing credential contents. The private directory
/// has no unprivileged writers, so subsequent opens/renames cannot be raced by a user.
fn check_directory(path: &Path, owner: u32) -> anyhow::Result<()> {
    ensure!(path.is_absolute(), "credential directory must be absolute");
    for ancestor in path.ancestors() {
        let meta = std::fs::symlink_metadata(ancestor)?;
        ensure!(
            meta.is_dir() && meta.uid() == owner && meta.mode() & 0o022 == 0,
            "credential directory has unsafe ownership, permissions, or a symlink"
        );
    }
    ensure!(
        std::fs::metadata(path)?.mode() & 0o077 == 0,
        "credential directory must be private"
    );
    Ok(())
}

fn record_path(dir: &Path, uid: u32) -> PathBuf {
    dir.join(format!("{uid}.keyring"))
}

fn read_record(path: &Path, owner: u32) -> anyhow::Result<Option<Vec<u8>>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.uid() == owner && meta.mode() & 0o077 == 0 && meta.nlink() == 1,
        "credential record has unsafe ownership, permissions, or type"
    );
    let mut blob = Vec::new();
    file.take((MAX_BLOB + 1) as u64).read_to_end(&mut blob)?;
    ensure!(blob.len() <= MAX_BLOB, "credential record is too large");
    Ok(Some(blob))
}

fn write_record(dir: &Path, account: &Account, blob: &[u8]) -> anyhow::Result<()> {
    let mut random = [0; 8];
    getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("random filename unavailable"))?;
    let tmp = dir.join(format!(
        ".{}-{}.tmp",
        account.uid,
        u64::from_le_bytes(random)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(blob)?;
        file.sync_all()?;
        std::fs::rename(&tmp, record_path(dir, account.uid))?;
        File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

pub fn enroll(username: &str, password: &[u8]) -> anyhow::Result<()> {
    let account = Account::lookup(username)?;
    validate_password(password)?;
    let dir = Path::new(STORE_DIR);
    // /var/lib/gaze is provisioned by gazed's StateDirectory; do not create arbitrary parents.
    check_directory(dir.parent().context("missing parent")?, 0)?;
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    check_directory(dir, 0)?;
    let mut key = Zeroizing::new([0; 32]);
    getrandom::fill(key.as_mut()).map_err(|_| anyhow::anyhow!("random key unavailable"))?;
    let (public, private) = crate::tpm::seal_credential_key(&key)?;
    let blob = encrypt(&key, &account, password, &public, &private)?;
    // Verify sealing before replacing a working record; a failed setup leaves it intact.
    let _check = decrypt_with(&blob, &account, crate::tpm::unseal_credential_key)?;
    ensure!(
        Account::lookup(username)?.binding == account.binding,
        "account changed during enrollment; retry"
    );
    write_record(dir, &account, &blob)
}

/// Call only from trusted PAM code after face, liveness, and confirmation succeed.
/// A missing record is an unenrolled user; other failures request password fallback.
pub fn load(username: &str) -> anyhow::Result<Option<Secret>> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "keyring access requires root"
    );
    let dir = Path::new(STORE_DIR);
    if !dir.try_exists()? {
        return Ok(None);
    }
    check_directory(dir, 0)?;
    let uid = Account::uid(username)?;
    let Some(blob) = read_record(&record_path(dir, uid), 0)? else {
        return Ok(None);
    };
    let account = Account::lookup(username)?;
    let secret = decrypt_with(&blob, &account, crate::tpm::unseal_credential_key)?;
    ensure!(
        Account::lookup(username)?.binding == account.binding,
        "account changed during unlock"
    );
    Ok(Some(secret))
}

pub fn forget(username: &str) -> anyhow::Result<()> {
    let uid = Account::uid(username)?;
    let dir = Path::new(STORE_DIR);
    if !dir.try_exists()? {
        return Ok(());
    }
    check_directory(dir, 0)?;
    match std::fs::remove_file(record_path(dir, uid)) {
        Ok(()) => File::open(dir)?.sync_all().map_err(Into::into),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;

    const KEY: [u8; 32] = [7; 32];
    fn account() -> Account {
        Account::bound_to(1000, "alice", b"shadow-hash")
    }
    fn blob() -> Vec<u8> {
        encrypt(&KEY, &account(), b"test password", b"public", b"private").unwrap()
    }

    #[test]
    fn authenticated_record_round_trips_without_plaintext_on_disk() {
        let blob = blob();
        assert!(!blob.windows(13).any(|w| w == b"test password"));
        let secret = decrypt_with(&blob, &account(), |public, private| {
            assert_eq!(public, b"public");
            assert_eq!(private, b"private");
            Ok(KEY)
        })
        .unwrap();
        assert_eq!(secret.as_slice(), b"test password\0");
        assert_ne!(blob, self::blob(), "fresh nonce on every enrollment");
    }

    #[test]
    fn password_changes_uid_reuse_and_record_swaps_cannot_release_credentials() {
        for changed in [
            Account::bound_to(1000, "alice", b"new-hash"),
            Account::bound_to(1001, "alice", b"shadow-hash"),
            Account::bound_to(1000, "bob", b"shadow-hash"),
        ] {
            assert!(decrypt_with(&blob(), &changed, |_, _| Ok(KEY)).is_err());
        }
    }

    #[test]
    fn missing_reset_or_wrong_tpm_and_tampering_fail_closed() {
        assert!(
            decrypt_with(&blob(), &account(), |_, _| anyhow::bail!(
                "TPM missing/reset"
            ))
            .is_err()
        );
        assert!(decrypt_with(&blob(), &account(), |_, _| Ok([9; 32])).is_err());
        let mut corrupt = blob();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(decrypt_with(&corrupt, &account(), |_, _| Ok(KEY)).is_err());
    }

    #[test]
    fn malformed_records_are_rejected_before_touching_tpm() {
        let mut overflow = blob();
        overflow[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        for invalid in [
            vec![],
            b"plaintext password".to_vec(),
            overflow,
            vec![0; MAX_BLOB + 1],
        ] {
            let touched = Cell::new(false);
            assert!(
                decrypt_with(&invalid, &account(), |_, _| {
                    touched.set(true);
                    Ok(KEY)
                })
                .is_err()
            );
            assert!(!touched.get());
        }
        let blob = blob();
        for end in 0..blob.len() {
            assert!(decrypt_with(&blob[..end], &account(), |_, _| Ok(KEY)).is_err());
        }
    }

    #[test]
    fn password_limits_and_c_string_safety() {
        for password in [
            vec![],
            b"embedded\0nul".to_vec(),
            vec![b'x'; MAX_PASSWORD + 1],
        ] {
            assert!(encrypt(&KEY, &account(), &password, b"p", b"s").is_err());
        }
        let password = vec![b'x'; MAX_PASSWORD];
        let blob = encrypt(&KEY, &account(), &password, b"p", b"s").unwrap();
        assert_eq!(
            decrypt_with(&blob, &account(), |_, _| Ok(KEY))
                .unwrap()
                .len(),
            MAX_PASSWORD + 1
        );
    }

    #[test]
    fn records_are_atomic_private_replaceable_and_missing_is_unenrolled() {
        let dir = tempfile::tempdir().unwrap();
        let path = record_path(dir.path(), account().uid);
        let owner = unsafe { libc::geteuid() };
        assert!(read_record(&path, owner).unwrap().is_none());
        write_record(dir.path(), &account(), &blob()).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        let replacement = encrypt(&KEY, &account(), b"replacement", b"p", b"s").unwrap();
        write_record(dir.path(), &account(), &replacement).unwrap();
        assert_eq!(read_record(&path, owner).unwrap().unwrap(), replacement);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn unsafe_record_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = record_path(dir.path(), account().uid);
        let owner = unsafe { libc::geteuid() };
        write_record(dir.path(), &account(), &blob()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_record(&path, owner).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_record(&path, owner.wrapping_add(1)).is_err());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_record(&link, owner).is_err());
        std::fs::remove_file(&link).unwrap();
        std::fs::hard_link(&path, &link).unwrap();
        assert!(read_record(&path, owner).is_err());
        assert!(read_record(dir.path(), owner).is_err());
    }

    #[test]
    #[ignore = "requires two disposable swtpm instances in TEST_TCTI and GAZE_TEST_OTHER_TCTI"]
    fn software_tpm_record_roundtrip_and_machine_binding() {
        use std::str::FromStr;
        use tss_esapi::{Context, TctiNameConf};
        let tcti = TctiNameConf::from_environment_variable().unwrap();
        let other =
            TctiNameConf::from_str(&std::env::var("GAZE_TEST_OTHER_TCTI").unwrap()).unwrap();
        assert!(matches!(tcti, TctiNameConf::Swtpm(_)));
        assert!(matches!(other, TctiNameConf::Swtpm(_)));
        let mut context = Context::new(tcti.clone()).unwrap();
        let mut second_tpm = Context::new(other).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (public, private) = crate::tpm::seal_credential_key_in(&mut context, &KEY).unwrap();
        let encrypted = encrypt(
            &KEY,
            &account(),
            b"synthetic keyring password",
            &public,
            &private,
        )
        .unwrap();
        write_record(dir.path(), &account(), &encrypted).unwrap();
        drop(context);
        let mut context = Context::new(tcti).unwrap();
        let on_disk = read_record(&record_path(dir.path(), account().uid), unsafe {
            libc::geteuid()
        })
        .unwrap()
        .unwrap();
        let secret = decrypt_with(&on_disk, &account(), |p, s| {
            crate::tpm::unseal_credential_key_in(&mut context, p, s)
        })
        .unwrap();
        assert_eq!(secret.as_slice(), b"synthetic keyring password\0");
        assert!(
            decrypt_with(&on_disk, &account(), |p, s| {
                crate::tpm::unseal_credential_key_in(&mut second_tpm, p, s)
            })
            .is_err()
        );
        // Failed unseal does not rewrite the enrollment.
        assert_eq!(
            std::fs::read(record_path(dir.path(), account().uid)).unwrap(),
            encrypted
        );
    }
}
