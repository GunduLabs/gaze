// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! The TPM 2.0 sealing primitives shared by gazed's template key and the keyring credential.
//!
//! `local_device_context` deliberately opens a local device instead of accepting a TCTI from
//! the PAM process environment. gazed builds its own context and keeps the environment
//! override its tests rely on.

use anyhow::{Context as _, anyhow};
use std::path::Path;
use std::str::FromStr;
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
use tss_esapi::tcti_ldr::DeviceConfig;
use tss_esapi::traits::{Marshall, UnMarshall};
use tss_esapi::{Context, TctiNameConf};
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;

/// A sealed key never leaves this crate as a bare array: the caller's copy wipes on drop.
pub type SealedKey = Zeroizing<[u8; KEY_LEN]>;

pub const TPM_RM_DEVICE: &str = "/dev/tpmrm0";
pub const TPM_RAW_DEVICE: &str = "/dev/tpm0";
pub const TPM_DEVICES: [&str; 2] = [TPM_RM_DEVICE, TPM_RAW_DEVICE];

pub fn local_device_context() -> anyhow::Result<Context> {
    for device in TPM_DEVICES {
        if Path::new(device).exists() {
            let config = DeviceConfig::from_str(device)?;
            if let Ok(context) = Context::new(TctiNameConf::Device(config)) {
                return Ok(context);
            }
        }
    }
    Err(anyhow!("no usable local TPM 2.0 device"))
}

// Nothing about this parent is stored on disk. CreatePrimary re-derives the identical key
// from the owner seed and this exact template, and no PCR policy means updates don't break it.
pub fn create_primary(context: &mut Context) -> anyhow::Result<tss_esapi::handles::KeyHandle> {
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
    let params = PublicEccParametersBuilder::new()
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
    let public = PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::Ecc)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(attrs)
        .with_ecc_parameters(params)
        .with_ecc_unique_identifier(EccPoint::default())
        .build()
        .context("failed to build primary public template")?;
    Ok(context
        .execute_with_nullauth_session(|ctx| {
            ctx.create_primary(Hierarchy::Owner, public, None, None, None, None)
        })
        .context("TPM CreatePrimary failed")?
        .key_handle)
}

pub fn sealed_object_public() -> anyhow::Result<Public> {
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

pub fn seal_in(context: &mut Context, key: &[u8; KEY_LEN]) -> anyhow::Result<(Public, Private)> {
    let parent = create_primary(context)?;
    let public = sealed_object_public()?;
    let sensitive =
        SensitiveData::try_from(key.to_vec()).map_err(|e| anyhow!("invalid key length: {e}"))?;
    let result = context.execute_with_nullauth_session(|ctx| {
        ctx.create(parent, public, None, Some(sensitive), None, None)
    });
    let _ = context.flush_context(parent.into());
    let created = result.context("TPM Create (seal) failed")?;
    Ok((created.out_public, created.out_private))
}

pub fn unseal_in(
    context: &mut Context,
    public: Public,
    private: Private,
) -> anyhow::Result<SealedKey> {
    let parent = create_primary(context)?;
    let result = context.execute_with_nullauth_session(|ctx| {
        let object = ctx.load(parent, private, public)?;
        let data = ctx.unseal(object.into());
        let _ = ctx.flush_context(object.into());
        data
    });
    let _ = context.flush_context(parent.into());

    // SensitiveData holds a Zeroizing buffer, so the only copy to guard is the one we return.
    let sensitive = result.context("TPM Load/Unseal failed")?;
    let bytes = sensitive.value();
    let key: [u8; KEY_LEN] = bytes.try_into().map_err(|_| {
        anyhow!(
            "unsealed key has unexpected length {} (expected {KEY_LEN})",
            bytes.len()
        )
    })?;
    Ok(Zeroizing::new(key))
}

pub fn seal(key: &[u8; KEY_LEN]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let mut context = local_device_context()?;
    let (public, private) = seal_in(&mut context, key)?;
    Ok((public.marshall()?, private.value().to_vec()))
}

pub fn unseal(public: &[u8], private: &[u8]) -> anyhow::Result<SealedKey> {
    let mut context = local_device_context()?;
    unseal_in(
        &mut context,
        Public::unmarshall(public)?,
        Private::try_from(private.to_vec())?,
    )
}
