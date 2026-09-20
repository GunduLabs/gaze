// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

/// Exit status `gazed` uses when it stops because the CPU cannot run inference.
/// `packaging/config/gazed.service` lists this in `RestartPreventExitStatus=`, so
/// changing it here without changing the unit reintroduces the crash loop.
pub const EXIT_UNSUPPORTED_CPU: u8 = 78;

pub const UNSUPPORTED_CPU_MESSAGE: &str = "AVX2 is unavailable; gazed cannot run on this CPU";

pub const UNSUPPORTED_CPU_FIX: &str =
    "Use a machine with AVX2 support. The CLI can run here, but the daemon cannot.";

/// Whether the prebuilt ONNX Runtime that `gazed` links can execute here. Its startup
/// code issues AVX2 unconditionally, so a miss is SIGILL rather than a recoverable error.
pub fn supports_inference() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::arch::is_x86_feature_detected!("avx2")
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unit hardcodes the exit status, so a change here silently reintroduces the
    /// SIGILL restart loop unless the packaging changes with it.
    #[test]
    fn the_packaged_unit_prevents_restarts_on_the_unsupported_cpu_status() {
        let unit = include_str!("../../packaging/config/gazed.service");
        assert!(
            unit.contains(&format!("RestartPreventExitStatus={EXIT_UNSUPPORTED_CPU}")),
            "packaging/config/gazed.service must set RestartPreventExitStatus={EXIT_UNSUPPORTED_CPU}"
        );

        let nix = include_str!("../../packaging/nix/nixos-module.nix");
        assert!(
            nix.contains(&format!(
                "RestartPreventExitStatus = {EXIT_UNSUPPORTED_CPU};"
            )),
            "packaging/nix/nixos-module.nix must set RestartPreventExitStatus = {EXIT_UNSUPPORTED_CPU};"
        );
    }

    #[cfg(all(target_os = "linux", any(target_arch = "x86", target_arch = "x86_64")))]
    #[test]
    fn inference_support_tracks_the_avx2_flag_the_installers_grep_for() {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap();
        let advertised = cpuinfo
            .lines()
            .filter(|line| line.starts_with("flags"))
            .any(|line| line.split_whitespace().any(|flag| flag == "avx2"));
        assert_eq!(advertised, supports_inference());
    }
}
