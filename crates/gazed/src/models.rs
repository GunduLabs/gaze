// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gaze_vision::inference::ModelFile;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

const RELEASE_BASE: &str = "https://github.com/deepinsight/insightface/releases/download/v0.7";
const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const BUFFALO_SC_SHA256: &str = "57d31b56b6ffa911c8a73cfc1707c73cab76efe7f13b675a05223bf42de47c72";
const BUFFALO_L_SHA256: &str = "80ffe37d8a5940d59a7384c201a2a38d4741f2f3c51eef46ebb28218a7b0ca2f";
const DET_500M_SHA256: &str = "5e4447f50245bbd7966bd6c0fa52938c61474a04ec7def48753668a9d8b4ea3a";
const W600K_MBF_SHA256: &str = "9cc6e4a75f0e2bf0b1aed94578f144d15175f357bdc05e815e5c4a02b319eb4f";
const DET_10G_SHA256: &str = "5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91";
const W600K_R50_SHA256: &str = "4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43";

pub const LIVENESS_MODEL_NAME: &str = "minifasnet_v2.onnx";
const LIVENESS_MODEL_SHA256: &str =
    "d7b3cd9ba8a7ceb13baa8c4720902e27ca3112eff52f926c08804af6b6eecc7b";
const LIVENESS_MODEL_URL: &str = "https://huggingface.co/garciafido/minifasnet-v2-anti-spoofing-onnx/resolve/main/minifasnet_v2.onnx";

pub const EYE_STATE_MODEL_NAME: &str = "open_closed_eye.onnx";
const EYE_STATE_MODEL_SHA256: &str =
    "4daa100034482525a26c9afb9297c16580a531189e66e3d2b2ac7d32becfd593";
const EYE_STATE_MODEL_URL: &str = "https://storage.openvinotoolkit.org/repositories/open_model_zoo/public/2022.1/open-closed-eye-0001/open_closed_eye.onnx";

fn zip_url(pack_name: &str) -> String {
    format!("{}/{}.zip", RELEASE_BASE, pack_name)
}

fn expected_pack_sha256(pack_name: &str) -> anyhow::Result<&'static str> {
    match pack_name {
        "buffalo_sc" => Ok(BUFFALO_SC_SHA256),
        "buffalo_l" => Ok(BUFFALO_L_SHA256),
        _ => anyhow::bail!("unknown model pack '{pack_name}'"),
    }
}

fn pack_for_model(model_name: &str) -> anyhow::Result<&'static str> {
    match model_name {
        "det_500m.onnx" | "w600k_mbf.onnx" => Ok("buffalo_sc"),
        "det_10g.onnx" | "w600k_r50.onnx" => Ok("buffalo_l"),
        _ => anyhow::bail!("no known model pack provides '{model_name}'"),
    }
}

fn required_packs<'a>(
    models: impl IntoIterator<Item = (&'a str, bool)>,
) -> anyhow::Result<Vec<&'static str>> {
    let mut packs = Vec::new();
    for (model_name, exists) in models {
        if !exists {
            let pack = pack_for_model(model_name)?;
            if !packs.contains(&pack) {
                packs.push(pack);
            }
        }
    }
    Ok(packs)
}

fn expected_model_sha256(model_name: &str) -> Option<&'static str> {
    match model_name {
        "det_500m.onnx" => Some(DET_500M_SHA256),
        "w600k_mbf.onnx" => Some(W600K_MBF_SHA256),
        "det_10g.onnx" => Some(DET_10G_SHA256),
        "w600k_r50.onnx" => Some(W600K_R50_SHA256),
        LIVENESS_MODEL_NAME => Some(LIVENESS_MODEL_SHA256),
        EYE_STATE_MODEL_NAME => Some(EYE_STATE_MODEL_SHA256),
        _ => None,
    }
}

fn ensure_private_dir(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        anyhow::bail!("{} is not a private directory", path.display());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn validate_model_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name.trim() != name
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.chars().any(char::is_control)
        || !name.ends_with(".onnx")
    {
        anyhow::bail!("model name must be a single .onnx file name");
    }
    Ok(())
}

fn cached_model(path: &Path, name: &str) -> anyhow::Result<Option<ModelFile>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let model = ModelFile::verify(path, None)?;
    if let Some(expected) = expected_model_sha256(name)
        && model.sha256() != expected
    {
        warn!(path = %path.display(), "Cached model checksum mismatch; downloading a replacement");
        return Ok(None);
    }
    Ok(Some(model))
}

struct DownloadTemp(PathBuf);

impl Drop for DownloadTemp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn download_file(url: &str, dest: &Path, expected_sha256: &str) -> anyhow::Result<ModelFile> {
    info!(url, timeout = ?MODEL_DOWNLOAD_TIMEOUT, "Downloading model file");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(MODEL_DOWNLOAD_TIMEOUT))
        .build()
        .into();
    let resp = agent.get(url).call()?;
    save_download(resp.into_body().into_reader(), dest, expected_sha256)
}

fn save_download(
    reader: impl Read,
    dest: &Path,
    expected_sha256: &str,
) -> anyhow::Result<ModelFile> {
    save_model(reader, dest, Some(expected_sha256))
}

fn save_model(
    mut reader: impl Read,
    dest: &Path,
    expected: Option<&str>,
) -> anyhow::Result<ModelFile> {
    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid model file path"))?;
    let tmp_path = dest.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp_path)?;
    // Only clean up files we created.
    let temp = DownloadTemp(tmp_path);
    std::io::copy(&mut reader, &mut file)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    let model = ModelFile::verify(&temp.0, expected)?;
    let model = model.rename(dest)?;
    fs::File::open(
        dest.parent()
            .ok_or_else(|| anyhow::anyhow!("model has no parent directory"))?,
    )?
    .sync_all()?;
    Ok(model)
}

fn extract_onnx_from_zip(
    zip_path: &Path,
    dest_dir: &Path,
    wanted: &[&str],
) -> anyhow::Result<Vec<ModelFile>> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut extracted = Vec::new();

    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx)?;
        let name = entry.name()?.into_owned();
        if name.ends_with(".onnx") {
            let basename = Path::new(&name)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            validate_model_name(&basename)?;
            let out_path = dest_dir.join(&basename);
            if !wanted.contains(&basename.as_str()) {
                continue;
            }
            if let Some(model) = cached_model(&out_path, &basename)? {
                debug!(file = %basename, "Verified model already exists, skipping extraction");
                extracted.push(model);
                continue;
            }
            extracted.push(save_model(
                &mut entry,
                &out_path,
                expected_model_sha256(&basename),
            )?);
            debug!(file = %basename, "Extracted model");
        }
    }

    Ok(extracted)
}

pub fn ensure_models(
    models_dir: &str,
    detector_name: &str,
    recognizer_name: &str,
) -> anyhow::Result<(ModelFile, ModelFile)> {
    ensure_models_with(models_dir, detector_name, recognizer_name, download_file)
}

fn ensure_models_with(
    models_dir: &str,
    detector_name: &str,
    recognizer_name: &str,
    mut download: impl FnMut(&str, &Path, &str) -> anyhow::Result<ModelFile>,
) -> anyhow::Result<(ModelFile, ModelFile)> {
    validate_model_name(detector_name)?;
    validate_model_name(recognizer_name)?;

    let dir = Path::new(models_dir);
    ensure_private_dir(dir)?;

    let det_path = dir.join(detector_name);
    let rec_path = dir.join(recognizer_name);

    let mut detector = cached_model(&det_path, detector_name)?;
    let mut recognizer = cached_model(&rec_path, recognizer_name)?;
    let needed_packs = required_packs([
        (detector_name, detector.is_some()),
        (recognizer_name, recognizer.is_some()),
    ])?;
    let wanted: Vec<&str> = [
        (detector_name, detector.is_none()),
        (recognizer_name, recognizer.is_none()),
    ]
    .into_iter()
    .filter_map(|(name, needed)| needed.then_some(name))
    .collect();

    for pack_name in needed_packs {
        let url = zip_url(pack_name);
        let zip_path = dir.join(format!("{pack_name}.zip"));
        let expected_sha256 = expected_pack_sha256(pack_name)?;

        download(&url, &zip_path, expected_sha256)?;
        let _archive = DownloadTemp(zip_path.clone());
        for model in extract_onnx_from_zip(&zip_path, dir, &wanted)? {
            if model.path() == det_path {
                detector = Some(model.clone());
            }
            if model.path() == rec_path {
                recognizer = Some(model);
            }
        }
    }

    Ok((
        detector.ok_or_else(|| {
            anyhow::anyhow!("Detection model '{detector_name}' not found in pack")
        })?,
        recognizer.ok_or_else(|| {
            anyhow::anyhow!("Recognition model '{recognizer_name}' not found in pack")
        })?,
    ))
}

pub fn ensure_liveness_model(models_dir: &str) -> anyhow::Result<ModelFile> {
    let dir = Path::new(models_dir);
    ensure_private_dir(dir)?;

    let path = dir.join(LIVENESS_MODEL_NAME);
    if let Some(model) = cached_model(&path, LIVENESS_MODEL_NAME)? {
        return Ok(model);
    }
    download_file(LIVENESS_MODEL_URL, &path, LIVENESS_MODEL_SHA256)
}

pub fn ensure_eye_state_model(models_dir: &str) -> anyhow::Result<ModelFile> {
    let dir = Path::new(models_dir);
    ensure_private_dir(dir)?;

    let path = dir.join(EYE_STATE_MODEL_NAME);
    if let Some(model) = cached_model(&path, EYE_STATE_MODEL_NAME)? {
        return Ok(model);
    }
    download_file(EYE_STATE_MODEL_URL, &path, EYE_STATE_MODEL_SHA256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "gaze-models-test-{}-{}-{name}",
                std::process::id(),
                unique
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn timed_out_download_is_cleaned_up_and_can_be_retried_in_the_same_process() {
        struct TimeoutAfterChunk(bool);
        impl Read for TimeoutAfterChunk {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "body stalled",
                    ));
                }
                self.0 = true;
                let n = buf.len().min(3);
                buf[..n].copy_from_slice(&b"abc"[..n]);
                Ok(n)
            }
        }

        let temp = TempDir::new("download-timeout");
        let dest = temp.path().join("model.onnx");
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let err = save_download(TimeoutAfterChunk(false), &dest, expected).unwrap_err();
        assert_eq!(
            err.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::TimedOut,
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);

        save_download(&b"abc"[..], &dest, expected).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"abc");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        assert_eq!(
            fs::metadata(dest).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn checksum_failure_cleans_up_without_replacing_an_existing_model() {
        let temp = TempDir::new("download-checksum");
        let dest = temp.path().join("model.onnx");
        fs::write(&dest, b"existing model").unwrap();
        assert!(save_download(&b"abc"[..], &dest, DET_500M_SHA256).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"existing model");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_download_rename_also_cleans_up_the_temp_file() {
        let temp = TempDir::new("download-rename");
        let dest = temp.path().join("model.onnx");
        fs::create_dir(&dest).unwrap();
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(save_download(&b"abc"[..], &dest, expected).is_err());
        assert!(dest.is_dir());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn legacy_pid_temp_files_do_not_prevent_a_new_download() {
        let temp = TempDir::new("download-legacy-temp");
        let dest = temp.path().join("model.onnx");
        let stale = temp
            .path()
            .join(format!(".model.onnx.{}.tmp", std::process::id()));
        fs::write(&stale, b"partial").unwrap();
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        save_download(&b"abc"[..], &dest, expected).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"abc");
        assert_eq!(fs::read(&stale).unwrap(), b"partial");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn corrupt_cached_models_request_a_new_pack_without_deleting_the_old_file() {
        let temp = TempDir::new("corrupt-cache");
        let detector = temp.path().join("det_500m.onnx");
        fs::write(&detector, b"interrupted extraction").unwrap();
        fs::write(temp.path().join("custom.onnx"), b"recognizer").unwrap();
        let mut requested = false;
        let result = ensure_models_with(
            temp.path().to_str().unwrap(),
            "det_500m.onnx",
            "custom.onnx",
            |url, _, hash| {
                requested = true;
                assert_eq!(url, zip_url("buffalo_sc"));
                assert_eq!(hash, BUFFALO_SC_SHA256);
                anyhow::bail!("offline test")
            },
        );
        assert!(result.is_err());
        assert!(requested);
        assert_eq!(fs::read(detector).unwrap(), b"interrupted extraction");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn healthy_cached_models_return_reusable_verified_digests_without_a_download() {
        let temp = TempDir::new("healthy-cache");
        fs::write(temp.path().join("detector.onnx"), b"abc").unwrap();
        fs::write(temp.path().join("recognizer.onnx"), b"abc").unwrap();
        let (detector, recognizer) = ensure_models_with(
            temp.path().to_str().unwrap(),
            "detector.onnx",
            "recognizer.onnx",
            |_, _, _| panic!("healthy cached models must not be downloaded"),
        )
        .unwrap();
        fs::remove_file(detector.path()).unwrap();
        fs::remove_file(recognizer.path()).unwrap();
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(detector.sha256(), expected);
        assert_eq!(recognizer.sha256(), expected);
    }

    #[test]
    fn a_verified_download_atomically_replaces_corrupt_bytes() {
        let temp = TempDir::new("replace-corrupt");
        let destination = temp.path().join("model.onnx");
        fs::write(&destination, b"partial").unwrap();
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let model = save_download(&b"abc"[..], &destination, expected).unwrap();
        assert_eq!(model.path(), destination);
        assert_eq!(model.sha256(), expected);
        assert_eq!(fs::read(destination).unwrap(), b"abc");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn invalid_extracted_bytes_do_not_replace_a_corrupt_known_model() {
        let temp = TempDir::new("bad-extraction-hash");
        let destination = temp.path().join("models");
        fs::create_dir(&destination).unwrap();
        let detector = destination.join("det_500m.onnx");
        fs::write(&detector, b"old partial").unwrap();
        let archive = temp.path().join("models.zip");
        write_zip(&archive, &[("det_500m.onnx", b"untrusted replacement")]);
        assert!(extract_onnx_from_zip(&archive, &destination, &["det_500m.onnx"]).is_err());
        assert_eq!(fs::read(detector).unwrap(), b"old partial");
        assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
    }

    #[test]
    fn a_crc_failure_never_publishes_a_partially_extracted_model() {
        let temp = TempDir::new("bad-zip-crc");
        let destination = temp.path().join("models");
        fs::create_dir(&destination).unwrap();
        let archive = temp.path().join("models.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file(
            "custom.onnx",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        let body = b"gaze-test-body";
        zip.write_all(body).unwrap();
        zip.finish().unwrap();
        let mut bytes = fs::read(&archive).unwrap();
        let offset = bytes
            .windows(body.len())
            .position(|bytes| bytes == body)
            .unwrap();
        bytes[offset] ^= 1;
        fs::write(&archive, bytes).unwrap();
        assert!(extract_onnx_from_zip(&archive, &destination, &["custom.onnx"]).is_err());
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }

    #[test]
    fn extraction_ignores_models_that_were_not_requested() {
        let temp = TempDir::new("wanted-models");
        let archive = temp.path().join("models.zip");
        let destination = temp.path().join("models");
        fs::create_dir(&destination).unwrap();
        write_zip(
            &archive,
            &[("wanted.onnx", b"wanted"), ("unused.onnx", b"unused")],
        );
        assert_eq!(
            extract_onnx_from_zip(&archive, &destination, &["wanted.onnx"])
                .unwrap()
                .len(),
            1
        );
        assert!(!destination.join("unused.onnx").exists());
    }

    #[test]
    fn model_name_validation_accepts_safe_onnx_files_only() {
        validate_model_name("det_500m.onnx").unwrap();
        validate_model_name("w600k-r50.v2.onnx").unwrap();

        for name in [
            "",
            " model.onnx",
            "model.onnx ",
            ".",
            "..",
            "../model.onnx",
            "dir/model.onnx",
            "dir\\model.onnx",
            "model.bin",
            "model.onnx\0bad",
            "model.onnx\n",
        ] {
            assert!(
                validate_model_name(name).is_err(),
                "{name:?} should be invalid"
            );
        }
    }

    #[test]
    fn expected_hash_tables_cover_known_models_and_packs() {
        assert_eq!(
            expected_pack_sha256("buffalo_sc").unwrap(),
            BUFFALO_SC_SHA256
        );
        assert_eq!(expected_pack_sha256("buffalo_l").unwrap(), BUFFALO_L_SHA256);
        assert!(expected_pack_sha256("unknown").is_err());

        assert_eq!(
            expected_model_sha256("det_500m.onnx"),
            Some(DET_500M_SHA256)
        );
        assert_eq!(
            expected_model_sha256("w600k_mbf.onnx"),
            Some(W600K_MBF_SHA256)
        );
        assert_eq!(expected_model_sha256("det_10g.onnx"), Some(DET_10G_SHA256));
        assert_eq!(
            expected_model_sha256("w600k_r50.onnx"),
            Some(W600K_R50_SHA256)
        );
        assert_eq!(
            expected_model_sha256(LIVENESS_MODEL_NAME),
            Some(LIVENESS_MODEL_SHA256)
        );
        assert_eq!(
            expected_model_sha256(EYE_STATE_MODEL_NAME),
            Some(EYE_STATE_MODEL_SHA256)
        );
        assert_eq!(expected_model_sha256("custom.onnx"), None);
    }

    #[test]
    fn pack_for_model_maps_every_quality_combination() {
        assert_eq!(pack_for_model("det_500m.onnx").unwrap(), "buffalo_sc");
        assert_eq!(pack_for_model("w600k_mbf.onnx").unwrap(), "buffalo_sc");
        assert_eq!(pack_for_model("det_10g.onnx").unwrap(), "buffalo_l");
        assert_eq!(pack_for_model("w600k_r50.onnx").unwrap(), "buffalo_l");

        for (detector, recognizer) in [
            ("det_500m.onnx", "w600k_mbf.onnx"),
            ("det_10g.onnx", "w600k_r50.onnx"),
            ("det_500m.onnx", "w600k_r50.onnx"),
            ("det_10g.onnx", "w600k_mbf.onnx"),
        ] {
            assert!(pack_for_model(detector).is_ok(), "{detector} has no pack");
            assert!(
                pack_for_model(recognizer).is_ok(),
                "{recognizer} has no pack"
            );
            assert!(expected_pack_sha256(pack_for_model(detector).unwrap()).is_ok());
            assert!(expected_pack_sha256(pack_for_model(recognizer).unwrap()).is_ok());
        }

        assert!(pack_for_model("custom.onnx").is_err());
    }

    #[test]
    fn mixed_quality_models_fetch_both_required_packs() {
        assert_eq!(
            required_packs([("det_500m.onnx", false), ("w600k_r50.onnx", false),]).unwrap(),
            vec!["buffalo_sc", "buffalo_l"]
        );
        assert_eq!(
            required_packs([("det_10g.onnx", false), ("w600k_mbf.onnx", false),]).unwrap(),
            vec!["buffalo_l", "buffalo_sc"]
        );
    }

    #[test]
    fn required_packs_skip_existing_models_and_deduplicate_downloads() {
        assert_eq!(
            required_packs([("det_500m.onnx", false), ("w600k_mbf.onnx", false),]).unwrap(),
            vec!["buffalo_sc"]
        );
        assert_eq!(
            required_packs([("det_500m.onnx", true), ("w600k_r50.onnx", false),]).unwrap(),
            vec!["buffalo_l"]
        );
        assert!(
            required_packs([("custom.onnx", false)]).is_err(),
            "a missing unknown model must not silently skip its download"
        );
    }

    #[test]
    fn sha256_verification_accepts_matching_digest_and_rejects_mismatch() {
        let temp = TempDir::new("sha256");
        let path = temp.path().join("input.bin");
        fs::write(&path, b"abc").unwrap();
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

        assert_eq!(ModelFile::verify(&path, None).unwrap().sha256(), expected);
        ModelFile::verify(&path, Some(expected)).unwrap();
        assert!(ModelFile::verify(&path, Some(DET_500M_SHA256)).is_err());
    }

    #[test]
    fn ensure_regular_model_rejects_directories_and_symlinks() {
        let temp = TempDir::new("regular-model");
        let file = temp.path().join("model.onnx");
        let dir = temp.path().join("dir.onnx");
        let symlink = temp.path().join("link.onnx");
        fs::write(&file, b"model").unwrap();
        fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&file, &symlink).unwrap();

        ModelFile::verify(&file, None).unwrap();
        assert!(ModelFile::verify(&dir, None).is_err());
        assert!(ModelFile::verify(&symlink, None).is_err());
    }

    #[test]
    fn extract_onnx_from_zip_strips_paths_and_ignores_non_onnx_entries() {
        let temp = TempDir::new("zip-extract");
        let zip_path = temp.path().join("models.zip");
        let dest = temp.path().join("models");
        fs::create_dir(&dest).unwrap();
        write_zip(
            &zip_path,
            &[
                ("nested/detector.onnx", b"detector"),
                ("recognizer.onnx", b"recognizer"),
                ("README.txt", b"ignore me"),
            ],
        );

        let extracted =
            extract_onnx_from_zip(&zip_path, &dest, &["detector.onnx", "recognizer.onnx"]).unwrap();

        assert_eq!(extracted.len(), 2);
        assert_eq!(fs::read(dest.join("detector.onnx")).unwrap(), b"detector");
        assert_eq!(
            fs::read(dest.join("recognizer.onnx")).unwrap(),
            b"recognizer"
        );
        assert!(!dest.join("README.txt").exists());
    }

    #[test]
    fn extract_onnx_from_zip_reuses_existing_regular_model() {
        let temp = TempDir::new("zip-existing");
        let zip_path = temp.path().join("models.zip");
        let dest = temp.path().join("models");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("custom.onnx"), b"existing").unwrap();
        write_zip(&zip_path, &[("custom.onnx", b"new")]);

        let extracted = extract_onnx_from_zip(&zip_path, &dest, &["custom.onnx"]).unwrap();

        assert_eq!(extracted[0].path(), dest.join("custom.onnx"));
        assert_eq!(fs::read(dest.join("custom.onnx")).unwrap(), b"existing");
    }
}
