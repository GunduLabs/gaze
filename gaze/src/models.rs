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

fn expected_model_sha256(model_name: &str) -> Option<&'static str> {
    match model_name {
        "det_500m.onnx" => Some(DET_500M_SHA256),
        "w600k_mbf.onnx" => Some(W600K_MBF_SHA256),
        "det_10g.onnx" => Some(DET_10G_SHA256),
        "w600k_r50.onnx" => Some(W600K_R50_SHA256),
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
    Ok(format!("{:x}", hasher.finalize()))
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
    info!(url, "Downloading model pack");
    let resp = ureq::get(url).call()?;
    let mut reader = resp.into_body().into_reader();
    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid model pack path"))?;
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

    let pack_name = match detector_name {
        d if d.contains("10g") => "buffalo_l",
        _ => "buffalo_sc",
    };

    let url = zip_url(pack_name);
    let zip_path = dir.join(format!("{}.zip", pack_name));
    let expected_sha256 = expected_pack_sha256(pack_name)?;

    download_file(&url, &zip_path, expected_sha256)?;
    extract_onnx_from_zip(&zip_path, dir)?;
    fs::remove_file(&zip_path)?;

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
