// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! The small TPM operation used only for the keyring credential record.
//!
//! It deliberately opens a local device instead of accepting a TCTI from the
//! PAM process environment.

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

const KEY_LEN: usize = 32;

fn context() -> anyhow::Result<Context> {
    for device in ["/dev/tpmrm0", "/dev/tpm0"] {
        if Path::new(device).exists() {
            let config = DeviceConfig::from_str(device)?;
            if let Ok(context) = Context::new(TctiNameConf::Device(config)) {
                return Ok(context);
            }
        }
    }
    Err(anyhow!("no usable local TPM 2.0 device"))
}

fn primary(context: &mut Context) -> anyhow::Result<tss_esapi::handles::KeyHandle> {
    let attrs = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_sensitive_data_origin(true)
        .with_user_with_auth(true)
        .with_decrypt(true)
        .with_sign_encrypt(false)
        .with_restricted(true)
        .build()?;
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
        .build()?;
    let public = PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::Ecc)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(attrs)
        .with_ecc_parameters(params)
        .with_ecc_unique_identifier(EccPoint::default())
        .build()?;
    Ok(context
        .execute_with_nullauth_session(|ctx| {
            ctx.create_primary(Hierarchy::Owner, public, None, None, None, None)
        })
        .context("TPM CreatePrimary failed")?
        .key_handle)
}

fn sealed_public() -> anyhow::Result<Public> {
    let attrs = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_user_with_auth(true)
        .with_sensitive_data_origin(false)
        .with_sign_encrypt(false)
        .with_decrypt(false)
        .with_restricted(false)
        .build()?;
    Ok(PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::KeyedHash)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(attrs)
        .with_keyed_hash_parameters(PublicKeyedHashParameters::new(KeyedHashScheme::Null))
        .with_keyed_hash_unique_identifier(Digest::default())
        .build()?)
}

pub fn seal(key: &[u8; KEY_LEN]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let mut context = context()?;
    let parent = primary(&mut context)?;
    let public = sealed_public()?;
    let result = context.execute_with_nullauth_session(|ctx| {
        ctx.create(
            parent,
            public,
            None,
            Some(SensitiveData::try_from(key.to_vec())?),
            None,
            None,
        )
    });
    let _ = context.flush_context(parent.into());
    let created = result.context("TPM Create failed")?;
    Ok((
        created.out_public.marshall()?,
        created.out_private.value().to_vec(),
    ))
}

pub fn unseal(public: &[u8], private: &[u8]) -> anyhow::Result<[u8; KEY_LEN]> {
    let mut context = context()?;
    let parent = primary(&mut context)?;
    let result = context.execute_with_nullauth_session(|ctx| {
        let object = ctx.load(
            parent,
            Private::try_from(private.to_vec())?,
            Public::unmarshall(public)?,
        )?;
        let result = ctx.unseal(object.into());
        let _ = ctx.flush_context(object.into());
        result
    });
    let _ = context.flush_context(parent.into());
    result
        .context("TPM Load/Unseal failed")?
        .value()
        .try_into()
        .map_err(|_| anyhow!("invalid credential key length"))
}
