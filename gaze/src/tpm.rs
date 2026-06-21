use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow};
use tss_esapi::attributes::ObjectAttributesBuilder;
use tss_esapi::interface_types::algorithm::{HashingAlgorithm, PublicAlgorithm};
use tss_esapi::interface_types::ecc::EccCurve;
use tss_esapi::interface_types::key_bits::AesKeyBits;
use tss_esapi::interface_types::resource_handles::Hierarchy;
use tss_esapi::structures::{
    Digest, EccPoint, EccScheme, KeyDerivationFunctionScheme, KeyedHashScheme, Private, Public,
    PublicBuilder, PublicEccParametersBuilder, PublicKeyedHashParameters, SensitiveData,
    SymmetricDefinitionObject,
};
use tss_esapi::traits::{Marshall, UnMarshall};
use tss_esapi::{Context, TctiNameConf};

use crate::crypto::KEY_LEN;

pub const STATE_DIR: &str = "/var/lib/gaze/tpm";

const PUB_FILE: &str = "dek.pub";
const PRIV_FILE: &str = "dek.priv";

pub fn load_or_create_dek(state_dir: &Path) -> anyhow::Result<[u8; KEY_LEN]> {
    if !tpm_device_present() && !tcti_override_present() {
        return Err(anyhow!(
            "no TPM device found (looked for /dev/tpmrm0 and /dev/tpm0)"
        ));
    }

    ensure_private_dir(state_dir).with_context(|| {
        format!(
            "failed to prepare TPM state directory {}",
            state_dir.display()
        )
    })?;

    let pub_path = state_dir.join(PUB_FILE);
    let priv_path = state_dir.join(PRIV_FILE);

    let mut context = build_context().context("failed to initialise TPM ESAPI context")?;

    if pub_path.exists() && priv_path.exists() {
        let public = Public::unmarshall(&std::fs::read(&pub_path)?)
            .context("failed to parse sealed TPM public blob")?;
        let private = Private::try_from(std::fs::read(&priv_path)?)
            .map_err(|e| anyhow!("failed to parse sealed TPM private blob: {e}"))?;
        let dek = unseal(&mut context, public, private).context(
            "could not unseal the template key; if the TPM was cleared, delete the TPM state \
             directory and re-enrol",
        )?;
        return Ok(dek);
    }

    let mut dek = [0u8; KEY_LEN];
    getrandom::fill(&mut dek)
        .map_err(|e| anyhow!("failed to draw a random data-encryption key: {e}"))?;

    let (public, private) = seal(&mut context, &dek).context("failed to seal the template key")?;
    write_private_file(&pub_path, &public.marshall()?)?;
    write_private_file(&priv_path, private.value())?;
    Ok(dek)
}

fn tpm_device_present() -> bool {
    Path::new("/dev/tpmrm0").exists() || Path::new("/dev/tpm0").exists()
}

fn tcti_override_present() -> bool {
    ["TPM2TOOLS_TCTI", "TCTI", "TEST_TCTI"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

fn build_context() -> anyhow::Result<Context> {
    let tcti = TctiNameConf::from_environment_variable()
        .unwrap_or_else(|_| TctiNameConf::Device(Default::default()));
    Ok(Context::new(tcti)?)
}

fn create_primary(context: &mut Context) -> anyhow::Result<tss_esapi::handles::KeyHandle> {
    let attrs = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_sensitive_data_origin(true)
        .with_user_with_auth(true)
        .with_decrypt(true)
        .with_sign_encrypt(false)
        .with_restricted(true)
        .build()
        .context("failed to build primary object attributes")?;

    let ecc_params = PublicEccParametersBuilder::new()
        .with_ecc_scheme(EccScheme::Null)
        .with_curve(EccCurve::NistP256)
        .with_is_signing_key(false)
        .with_is_decryption_key(true)
        .with_restricted(true)
        .with_symmetric(SymmetricDefinitionObject::Aes {
            key_bits: AesKeyBits::Aes128,
            mode: tss_esapi::interface_types::algorithm::SymmetricMode::Cfb,
        })
        .with_key_derivation_function_scheme(KeyDerivationFunctionScheme::Null)
        .build()
        .context("failed to build primary ECC parameters")?;

    // Owner hierarchy, no PCR policy: bound to this TPM but survives firmware/kernel updates.
    let public = PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::Ecc)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(attrs)
        .with_ecc_parameters(ecc_params)
        .with_ecc_unique_identifier(EccPoint::default())
        .build()
        .context("failed to build primary public template")?;

    let primary = context
        .execute_with_nullauth_session(|ctx| {
            ctx.create_primary(Hierarchy::Owner, public, None, None, None, None)
        })
        .context("TPM CreatePrimary failed")?;
    Ok(primary.key_handle)
}

fn sealed_object_public() -> anyhow::Result<Public> {
    let attrs = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_user_with_auth(true)
        // We supply the data, so it must not originate in the TPM.
        .with_sensitive_data_origin(false)
        .with_sign_encrypt(false)
        .with_decrypt(false)
        .with_restricted(false)
        .build()
        .context("failed to build sealed-object attributes")?;

    PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::KeyedHash)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(attrs)
        .with_keyed_hash_parameters(PublicKeyedHashParameters::new(KeyedHashScheme::Null))
        .with_keyed_hash_unique_identifier(Digest::default())
        .build()
        .context("failed to build sealed-object public template")
}

fn seal(context: &mut Context, dek: &[u8; KEY_LEN]) -> anyhow::Result<(Public, Private)> {
    let primary = create_primary(context)?;
    let public = sealed_object_public()?;
    let sensitive =
        SensitiveData::try_from(dek.to_vec()).map_err(|e| anyhow!("invalid DEK length: {e}"))?;

    let result = context.execute_with_nullauth_session(|ctx| {
        ctx.create(primary, public, None, Some(sensitive), None, None)
    });
    let _ = context.flush_context(primary.into());
    let created = result.context("TPM Create (seal) failed")?;
    Ok((created.out_public, created.out_private))
}

fn unseal(
    context: &mut Context,
    public: Public,
    private: Private,
) -> anyhow::Result<[u8; KEY_LEN]> {
    let primary = create_primary(context)?;
    let result = context.execute_with_nullauth_session(|ctx| {
        let object = ctx.load(primary, private, public)?;
        let data = ctx.unseal(object.into());
        let _ = ctx.flush_context(object.into());
        data
    });
    let _ = context.flush_context(primary.into());

    let sensitive = result.context("TPM Load/Unseal failed")?;
    let bytes = sensitive.value();
    let dek: [u8; KEY_LEN] = bytes.try_into().map_err(|_| {
        anyhow!(
            "unsealed key has unexpected length {} (expected {KEY_LEN})",
            bytes.len()
        )
    })?;
    Ok(dek)
}

fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a private directory",
            path.display()
        )));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent: PathBuf = path
        .parent()
        .map(Path::to_path_buf)
        .context("sealed key path has no parent directory")?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("dek"),
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("failed to create {}", tmp.display()))?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.flush()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("failed to write {}", tmp.display()));
    }
    drop(file);
    std::fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize TPM tests: they share one TPM with very few transient-object slots.
    static TPM_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn device_probe_does_not_panic() {
        let _ = tpm_device_present();
    }

    #[test]
    #[ignore = "requires a usable TPM"]
    fn seal_unseal_round_trip() {
        let _guard = TPM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("gaze-tpm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = load_or_create_dek(&dir).expect("seal");
        let second = load_or_create_dek(&dir).expect("unseal");
        assert_eq!(first, second, "reloading must yield the same DEK");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "requires a usable TPM"]
    fn full_daemon_workflow_encrypts_and_reloads() {
        use crate::crypto::EmbeddingCipher;
        use crate::users::UserDatabase;
        use gaze_core::face::Spectrum;
        use ndarray::Array1;

        let _guard = TPM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("gaze-tpm-wf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tpm_dir = root.join("tpm");
        let users_dir = root.join("users");
        let users = users_dir.to_str().unwrap();

        let dek = load_or_create_dek(&tpm_dir).expect("seal DEK");
        let mut db =
            UserDatabase::new_with_cipher(users, 4, Some(EmbeddingCipher::new(&dek))).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            vec![(Array1::from_vec(vec![0.1, 0.2, 0.3]), Spectrum::Rgb)],
        )
        .unwrap();

        let plain = UserDatabase::new(users, 4).unwrap();
        assert_eq!(plain.get_user_embeddings("alice").map(|v| v.len()), Some(0));

        let dek2 = load_or_create_dek(&tpm_dir).expect("unseal DEK");
        assert_eq!(dek, dek2);
        let db2 =
            UserDatabase::new_with_cipher(users, 4, Some(EmbeddingCipher::new(&dek2))).unwrap();
        assert_eq!(db2.get_user_embeddings("alice").unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }
}
