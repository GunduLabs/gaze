use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

const RELEASE_BASE: &str = "https://github.com/deepinsight/insightface/releases/download/v0.7";
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

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};

    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{:02x}", b);
        acc
    }))
}

fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
    let actual = sha256_file(path)?;
    if actual != expected {
        anyhow::bail!(
            "checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

fn ensure_regular_model(path: &Path) -> anyhow::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        anyhow::bail!("model path is not a regular file: {}", path.display());
    }
    Ok(())
}

fn verify_known_model(path: &Path, model_name: &str) -> anyhow::Result<()> {
    ensure_regular_model(path)?;
    if let Some(expected) = expected_model_sha256(model_name) {
        verify_sha256(path, expected)?;
    }
    Ok(())
}

fn download_file(url: &str, dest: &Path, expected_sha256: &str) -> anyhow::Result<()> {
    info!(url, "Downloading model file");
    let resp = ureq::get(url).call()?;
    let mut reader = resp.into_body().into_reader();
    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid model file path"))?;
    let tmp_path = dest.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp_path)?;
    std::io::copy(&mut reader, &mut file)?;
    file.flush()?;
    drop(file);
    if let Err(err) = verify_sha256(&tmp_path, expected_sha256) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    fs::rename(&tmp_path, dest)?;
    Ok(())
}

fn extract_onnx_from_zip(zip_path: &Path, dest_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut extracted = Vec::new();

    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx)?;
        let name = entry.name().to_string();
        if name.ends_with(".onnx") {
            let basename = Path::new(&name)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            validate_model_name(&basename)?;
            let out_path = dest_dir.join(&basename);
            if fs::symlink_metadata(&out_path).is_ok() {
                ensure_regular_model(&out_path)?;
                debug!(file = %basename, "Model already exists, skipping extraction");
                extracted.push(out_path);
                continue;
            }
            let mut out_file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
            out_file.flush()?;
            extracted.push(out_path);
            debug!(file = %basename, "Extracted model");
        }
    }

    Ok(extracted)
}

pub fn ensure_models(
    models_dir: &str,
    detector_name: &str,
    recognizer_name: &str,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    validate_model_name(detector_name)?;
    validate_model_name(recognizer_name)?;

    let dir = Path::new(models_dir);
    ensure_private_dir(dir)?;

    let det_path = dir.join(detector_name);
    let rec_path = dir.join(recognizer_name);

    if det_path.exists() && rec_path.exists() {
        verify_known_model(&det_path, detector_name)?;
        verify_known_model(&rec_path, recognizer_name)?;
        return Ok((det_path, rec_path));
    }

    let needed_packs = required_packs([
        (detector_name, det_path.exists()),
        (recognizer_name, rec_path.exists()),
    ])?;

    for pack_name in needed_packs {
        let url = zip_url(pack_name);
        let zip_path = dir.join(format!("{}.zip", pack_name));
        let expected_sha256 = expected_pack_sha256(pack_name)?;

        download_file(&url, &zip_path, expected_sha256)?;
        extract_onnx_from_zip(&zip_path, dir)?;
        fs::remove_file(&zip_path)?;
    }

    if !det_path.exists() {
        anyhow::bail!("Detection model '{}' not found in pack", detector_name);
    }
    if !rec_path.exists() {
        anyhow::bail!("Recognition model '{}' not found in pack", recognizer_name);
    }
    verify_known_model(&det_path, detector_name)?;
    verify_known_model(&rec_path, recognizer_name)?;

    Ok((det_path, rec_path))
}

pub fn ensure_liveness_model(models_dir: &str) -> anyhow::Result<PathBuf> {
    let dir = Path::new(models_dir);
    ensure_private_dir(dir)?;

    let path = dir.join(LIVENESS_MODEL_NAME);
    if path.exists() {
        verify_known_model(&path, LIVENESS_MODEL_NAME)?;
        return Ok(path);
    }

    download_file(LIVENESS_MODEL_URL, &path, LIVENESS_MODEL_SHA256)?;
    Ok(path)
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

        assert_eq!(sha256_file(&path).unwrap(), expected);
        verify_sha256(&path, expected).unwrap();
        assert!(verify_sha256(&path, DET_500M_SHA256).is_err());
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

        ensure_regular_model(&file).unwrap();
        assert!(ensure_regular_model(&dir).is_err());
        assert!(ensure_regular_model(&symlink).is_err());
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
                ("nested/det_500m.onnx", b"detector"),
                ("w600k_mbf.onnx", b"recognizer"),
                ("README.txt", b"ignore me"),
            ],
        );

        let mut extracted = extract_onnx_from_zip(&zip_path, &dest).unwrap();
        extracted.sort();

        assert_eq!(extracted.len(), 2);
        assert_eq!(fs::read(dest.join("det_500m.onnx")).unwrap(), b"detector");
        assert_eq!(
            fs::read(dest.join("w600k_mbf.onnx")).unwrap(),
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
        fs::write(dest.join("det_500m.onnx"), b"existing").unwrap();
        write_zip(&zip_path, &[("det_500m.onnx", b"new")]);

        let extracted = extract_onnx_from_zip(&zip_path, &dest).unwrap();

        assert_eq!(extracted, vec![dest.join("det_500m.onnx")]);
        assert_eq!(fs::read(dest.join("det_500m.onnx")).unwrap(), b"existing");
    }
}
