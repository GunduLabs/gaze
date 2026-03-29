use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

const RELEASE_BASE: &str = "https://github.com/deepinsight/insightface/releases/download/v0.7";

fn zip_url(pack_name: &str) -> String {
    format!("{}/{}.zip", RELEASE_BASE, pack_name)
}

fn download_file(url: &str, dest: &Path) -> anyhow::Result<()> {
    info!(url, "Downloading model pack");
    let resp = ureq::get(url).call()?;
    let mut reader = resp.into_body().into_reader();
    let mut file = fs::File::create(dest)?;
    std::io::copy(&mut reader, &mut file)?;
    file.flush()?;
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
            let out_path = dest_dir.join(&basename);
            let mut out_file = fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
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
    let dir = Path::new(models_dir);
    fs::create_dir_all(dir)?;

    let det_path = dir.join(detector_name);
    let rec_path = dir.join(recognizer_name);

    if det_path.exists() && rec_path.exists() {
        return Ok((det_path, rec_path));
    }

    let pack_name = match detector_name {
        d if d.contains("10g") => "buffalo_l",
        _ => "buffalo_sc",
    };

    let url = zip_url(pack_name);
    let zip_path = dir.join(format!("{}.zip", pack_name));

    download_file(&url, &zip_path)?;
    extract_onnx_from_zip(&zip_path, dir)?;
    fs::remove_file(&zip_path)?;

    if !det_path.exists() {
        anyhow::bail!("Detection model '{}' not found in pack", detector_name);
    }
    if !rec_path.exists() {
        anyhow::bail!("Recognition model '{}' not found in pack", recognizer_name);
    }

    Ok((det_path, rec_path))
}
