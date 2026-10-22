//! Checks on the player's PC (row L33): Tailscale, free disk space (`system.ts`) and the client-reported
//! device signals (`device-signals.ts`: neither the SMBIOS UUID nor the TPM key hash is attestation).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::launch::{run_hidden, system_root};

pub fn tailscale_candidates() -> Vec<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let on_path = std::env::split_paths(&path).filter(|dir| !dir.as_os_str().is_empty());
    if cfg!(target_os = "linux") {
        let mut out: Vec<PathBuf> = ["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/snap/bin/tailscale"]
            .map(PathBuf::from)
            .to_vec();
        for dir in on_path {
            let candidate = dir.join("tailscale");
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
        return out;
    }
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    for base in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .chain([PathBuf::from("C:\\Program Files")])
    {
        let dir = base.join("Tailscale");
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Tailscale"));
    }
    let mut out: Vec<PathBuf> = dirs.into_iter().map(|dir| dir.join("tailscale.exe")).collect();
    out.extend(on_path.map(|dir| PathBuf::from(dir.to_string_lossy().trim_matches('"')).join("tailscale.exe")));
    out
}

pub fn find_tailscale() -> Option<PathBuf> {
    tailscale_candidates().into_iter().find(|candidate| candidate.is_file())
}

/// Free space on the drive that holds `dir` (or its nearest existing parent).
pub fn free_bytes(dir: &Path) -> Option<u64> {
    let mut current = Some(dir);
    while let Some(path) = current {
        if let Ok(free) = fs4::available_space(path) {
            return Some(free);
        }
        current = path.parent();
    }
    None
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSignals {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smbios_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tpm_ek_sha256: Option<String>,
}

fn all_same(text: &str, c: u8) -> bool {
    text.bytes().all(|b| b == c)
}

/// Known formats only, without firmware placeholders.
pub fn normalize_device_signals(uuid: &str, tpm: &str) -> DeviceSignals {
    let uuid = uuid.trim().to_lowercase();
    let groups: Vec<&str> = uuid.split('-').collect();
    let shaped = groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_hexdigit()));
    let bare = uuid.replace('-', "");
    let uuid_ok =
        shaped && !all_same(&bare, b'0') && !all_same(&bare, b'f') && uuid != "03000200-0400-0500-0006-000700080009";
    let tpm = tpm.trim().to_lowercase();
    let tpm_ok =
        tpm.len() == 64 && tpm.bytes().all(|b| b.is_ascii_hexdigit()) && !all_same(&tpm, b'0') && !all_same(&tpm, b'f');
    DeviceSignals {
        smbios_uuid: uuid_ok.then_some(uuid),
        tpm_ek_sha256: tpm_ok.then_some(tpm),
    }
}

const PREFIX: &str = "$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue';try{";

/// Two fixed, hidden, bounded PowerShell queries; a missing, denied or slow TPM never drops the UUID.
pub async fn collect_device_signals() -> DeviceSignals {
    if !cfg!(windows) {
        return DeviceSignals::default();
    }
    let powershell = system_root()
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let read = |script: String| {
        let powershell = powershell.clone();
        async move {
            run_hidden(
                &powershell,
                &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &script],
                Duration::from_secs(4),
                4096,
            )
            .await
            .unwrap_or_default()
        }
    };
    let (uuid, tpm) = tokio::join!(
        read(format!(
            "{PREFIX}[string](Get-CimInstance -ClassName Win32_ComputerSystemProduct -ErrorAction Stop).UUID}}catch{{}}"
        )),
        read(format!(
            "{PREFIX}$ek=Get-TpmEndorsementKeyInfo -HashAlgorithm Sha256 -ErrorAction Stop;if($ek.IsPresent){{[string]$ek.PublicKeyHash}}}}catch{{}}"
        )),
    );
    normalize_device_signals(&uuid, &tpm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_signals_keep_known_formats_and_drop_placeholders() {
        let uuid = "12345678-1234-1234-1234-123456789abc";
        let tpm = "ab".repeat(32);
        assert_eq!(
            normalize_device_signals(&format!(" {}\r\n", uuid.to_uppercase()), &tpm.to_uppercase()),
            DeviceSignals {
                smbios_uuid: Some(uuid.into()),
                tpm_ek_sha256: Some(tpm.clone())
            }
        );
        for bad in [
            "00000000-0000-0000-0000-000000000000",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
            "03000200-0400-0500-0006-000700080009",
            "Default string",
            "unknown",
            "not-a-uuid",
        ] {
            assert_eq!(normalize_device_signals(bad, "").smbios_uuid, None, "{bad}");
        }
        for bad in [
            "0".repeat(64),
            "f".repeat(64),
            "g".repeat(64),
            "ab".into(),
            format!("{tpm}0"),
        ] {
            assert_eq!(normalize_device_signals("", &bad).tpm_ek_sha256, None, "{bad}");
        }
        assert_eq!(
            serde_json::to_value(normalize_device_signals(uuid, "")).unwrap(),
            serde_json::json!({ "smbiosUuid": uuid })
        );
    }

    #[test]
    fn free_space_of_a_missing_folder_is_its_parents() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_bytes(&dir.path().join("not").join("yet")).is_some_and(|free| free > 0));
    }

    #[test]
    fn tailscale_is_looked_for_in_the_usual_places() {
        let candidates = tailscale_candidates();
        if cfg!(windows) {
            assert!(
                candidates
                    .iter()
                    .any(|c| c.ends_with(Path::new("Tailscale").join("tailscale.exe")))
            );
        } else {
            assert_eq!(candidates[0], PathBuf::from("/usr/bin/tailscale"));
        }
    }
}
