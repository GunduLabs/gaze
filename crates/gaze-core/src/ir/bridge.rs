// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

const MAX_MARKER_BYTES: u64 = 4096;

#[derive(Debug, PartialEq, Eq)]
pub struct BridgeMarker {
    pub source: String,
    pub sink: Option<String>,
}

/// The marker is a trust anchor: it must be a regular file owned by root that group and others
/// cannot write, so only a privileged bridge service can create it.
pub fn read_bridge_marker(marker: &str) -> anyhow::Result<BridgeMarker> {
    let path = Path::new(marker);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        anyhow::bail!("bridge marker {marker} does not name a file");
    };
    let parent = std::fs::canonicalize(parent).map_err(|e| {
        anyhow::anyhow!("cannot resolve the directory of bridge marker {marker}: {e}")
    })?;
    if let Some(dir) = parent.ancestors().find(|dir| !root_controlled_dir(dir)) {
        anyhow::bail!(
            "bridge marker {marker} is under {}, which a user other than root can modify",
            dir.display()
        );
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(parent.join(name))
        .map_err(|e| {
            anyhow::anyhow!("cannot open bridge marker {marker}: {e}; is the bridge running?")
        })?;
    let metadata = file
        .metadata()
        .map_err(|e| anyhow::anyhow!("cannot stat bridge marker {marker}: {e}"))?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        anyhow::bail!(
            "bridge marker {marker} must be a regular file owned by root and writable only by \
             its owner"
        );
    }
    let mut content = String::new();
    (&mut file)
        .take(MAX_MARKER_BYTES)
        .read_to_string(&mut content)
        .map_err(|e| anyhow::anyhow!("cannot read bridge marker {marker}: {e}"))?;
    let mut lines = content.lines().map(str::trim);
    let source = marker_video_node(marker, lines.next().unwrap_or_default())?;
    let sink = lines
        .next()
        .filter(|line| !line.is_empty())
        .map(|line| marker_video_node(marker, line))
        .transpose()?;
    Ok(BridgeMarker { source, sink })
}

fn marker_video_node(marker: &str, line: &str) -> anyhow::Result<String> {
    let is_video_node = line
        .strip_prefix("/dev/video")
        .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()));
    if !is_video_node {
        anyhow::bail!("bridge marker {marker} names {line:?}, which is not a /dev/video* node");
    }
    if !std::fs::metadata(line).is_ok_and(|m| m.file_type().is_char_device()) {
        anyhow::bail!("bridge marker {marker} names {line}, which is not present");
    }
    Ok(line.to_string())
}

fn root_controlled_dir(dir: &Path) -> bool {
    std::fs::metadata(dir).is_ok_and(|m| {
        m.is_dir() && m.uid() == 0 && (m.mode() & 0o022 == 0 || m.mode() & 0o1000 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_bridge_marker_must_be_root_controlled_and_name_a_video_node() {
        let root = std::env::temp_dir().join(format!(
            "gaze-bridge-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let marker = root.join("marker");
        let marker = marker.to_str().unwrap();
        let as_root = unsafe { libc::geteuid() } == 0;
        let untrusted = |root_error: &str| {
            if as_root {
                root_error.to_string()
            } else {
                "other than root can modify".to_string()
            }
        };

        std::fs::write(marker, "not-a-node\n").unwrap();
        std::fs::set_permissions(marker, std::fs::Permissions::from_mode(0o600)).unwrap();
        let err = read_bridge_marker(marker).unwrap_err();
        assert!(
            err.to_string().contains(&untrusted("not a /dev/video* node")),
            "unexpected error: {err:#}"
        );

        if as_root {
            std::fs::write(marker, "/dev/video9999\n").unwrap();
            let err = read_bridge_marker(marker).unwrap_err();
            assert!(
                err.to_string().contains("not present"),
                "unexpected error: {err:#}"
            );

            let link = root.join("link");
            std::os::unix::fs::symlink(marker, &link).unwrap();
            assert!(read_bridge_marker(link.to_str().unwrap()).is_err());
        }

        std::fs::set_permissions(marker, std::fs::Permissions::from_mode(0o666)).unwrap();
        let err = read_bridge_marker(marker).unwrap_err();
        assert!(
            err.to_string().contains(&untrusted("writable only")),
            "unexpected error: {err:#}"
        );

        assert!(read_bridge_marker(&root.join("missing").to_string_lossy()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn marker_lines_must_be_video_nodes() {
        assert!(marker_video_node("m", "/dev/video").is_err());
        assert!(marker_video_node("m", "/dev/video4x").is_err());
        assert!(marker_video_node("m", "/dev/sda").is_err());
        assert!(
            marker_video_node("m", "/dev/video9999")
                .unwrap_err()
                .to_string()
                .contains("not present")
        );
    }

    #[test]
    fn root_controlled_dir_requires_an_existing_root_directory() {
        assert!(root_controlled_dir(Path::new("/")));
        assert!(!root_controlled_dir(Path::new("/nonexistent-gaze-dir")));
    }
}
