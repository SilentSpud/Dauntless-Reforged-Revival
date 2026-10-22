//! Self-update (row L46, D2). The Windows installer (NSIS) and the AppImage update themselves with
//! `tauri-plugin-updater` from `latest.json` in the `launcher-updates` release, and a bundle whose signature
//! is not this launcher's key (`updater.key.pub`) is refused. deb and rpm installs are their package
//! manager's: the launcher looks for a newer GitHub release and installs its package through `pkexec`
//! (`linux-updater.ts`). Packaged builds look once at start and then every hour, as the Electron launcher
//! did; nothing installs until the player presses the update button (`installUpdate`).

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::controller::ControllerHandle;
use crate::platform::log;

const RELEASES_API: &str = "https://api.github.com/repos/mixutin/dauntless-revived/releases/latest";
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    Deb,
    Rpm,
}

enum Pending {
    Bundle(Box<tauri_plugin_updater::Update>),
    Package {
        url: String,
        kind: PackageKind,
        version: String,
    },
}

/// The update found last, waiting for the player.
#[derive(Default)]
pub struct Updates {
    pending: tokio::sync::Mutex<Option<Pending>>,
}

/// Whether this install updates itself (the Windows installer or an AppImage) rather than through its
/// package manager.
pub fn self_updating() -> bool {
    cfg!(windows) || std::env::var_os("APPIMAGE").is_some()
}

/// `newer` in `linux-updater.ts`: dotted numbers, a missing part counting as 0.
pub fn newer(candidate: &str, current: &str) -> bool {
    let parts = |text: &str| -> Vec<u64> {
        text.split('.')
            .map(|part| {
                part.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (a, b) = (parts(candidate), parts(current));
    for index in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(index).copied().unwrap_or(0), b.get(index).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

#[derive(Deserialize)]
struct Release {
    tag_name: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: Option<String>,
    browser_download_url: Option<String>,
}

/// The package of a newer release for this kind of install, from the GitHub release's assets.
fn package_in(release: &Release, current: &str, kind: PackageKind) -> Option<(String, String)> {
    let version = release.tag_name.as_deref()?.strip_prefix("launcher-v")?.to_owned();
    if !newer(&version, current) {
        return None;
    }
    let suffix = match kind {
        PackageKind::Deb => ".deb",
        PackageKind::Rpm => ".rpm",
    };
    let asset = release.assets.iter().find(|asset| {
        asset
            .name
            .as_deref()
            .is_some_and(|name| name.contains(&format!("-{version}-linux-")) && name.ends_with(suffix))
    })?;
    Some((asset.browser_download_url.clone()?, version))
}

fn package_kind() -> Option<PackageKind> {
    if std::path::Path::new("/usr/bin/dpkg").exists() {
        Some(PackageKind::Deb)
    } else if std::path::Path::new("/usr/bin/rpm").exists() {
        Some(PackageKind::Rpm)
    } else {
        None
    }
}

fn client(app: &AppHandle) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("DauntlessRevivedLauncher/{}", app.package_info().version))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())
}

async fn check(app: &AppHandle) -> Result<Option<Pending>, String> {
    if self_updating() {
        let update = app
            .updater()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?;
        return Ok(update.map(|update| Pending::Bundle(Box::new(update))));
    }
    let Some(kind) = package_kind() else { return Ok(None) };
    let response = client(app)?
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("GitHub returned {}", response.status()));
    }
    let release: Release = response.json().await.map_err(|e| e.to_string())?;
    let current = app.package_info().version.to_string();
    Ok(package_in(&release, &current, kind).map(|(url, version)| Pending::Package { url, kind, version }))
}

/// Looks now and then every hour; marks the controller's snapshot when an update waits.
pub fn start(app: AppHandle, controller: ControllerHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            match check(&app).await {
                Ok(Some(pending)) => {
                    let version = match &pending {
                        Pending::Bundle(update) => update.version.clone(),
                        Pending::Package { version, .. } => version.clone(),
                    };
                    log::info(&format!("update: launcher {version} is ready to install"));
                    *app.state::<Arc<Updates>>().pending.lock().await = Some(pending);
                    controller.set_update_ready();
                }
                Ok(None) => {}
                Err(error) => log::warn(&format!("update: check failed: {error}")),
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// The player pressed the update button: installs the waiting update, then the launcher restarts or quits
/// (the Windows installer closes it itself).
pub async fn install(app: AppHandle, controller: ControllerHandle) {
    if !controller.may_install_update() {
        return;
    }
    let Some(pending) = app.state::<Arc<Updates>>().pending.lock().await.take() else {
        return;
    };
    let outcome = match pending {
        Pending::Bundle(update) => match update.download_and_install(|_, _| {}, || {}).await {
            Ok(()) => {
                log::info(&format!("update: launcher {} installed; restarting", update.version));
                app.restart();
            }
            Err(error) => Err(error.to_string()),
        },
        Pending::Package { url, kind, version } => install_package(&app, &url, kind, &version).await,
    };
    if let Err(error) = outcome {
        log::error(&format!("update: install failed: {error}"));
    }
}

async fn install_package(app: &AppHandle, url: &str, kind: PackageKind, version: &str) -> Result<(), String> {
    let response = client(app)?.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("update download failed ({})", response.status()));
    }
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join(format!("dauntless-revived-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let extension = match kind {
        PackageKind::Deb => "deb",
        PackageKind::Rpm => "rpm",
    };
    let file = dir.join(format!("DauntlessRevivedLauncher-{version}.{extension}"));
    std::fs::write(&file, &bytes).map_err(|e| e.to_string())?;
    // pkexec asks for the password; the package manager replaces this launcher, which quits for it
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = match kind {
        PackageKind::Deb => ("apt", vec!["install".as_ref(), "-y".as_ref(), file.as_os_str()]),
        PackageKind::Rpm => ("rpm", vec!["-U".as_ref(), file.as_os_str()]),
    };
    std::process::Command::new("pkexec")
        .arg(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    log::info(&format!(
        "update: installing launcher {version} through the package manager"
    ));
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_dotted_numbers() {
        assert!(newer("0.2.0", "0.1.39"));
        assert!(newer("0.1.40", "0.1.39"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.1.39", "0.1.39"));
        assert!(!newer("0.1.38", "0.1.39"));
        assert!(!newer("0.1", "0.1.0"));
    }

    #[test]
    fn the_package_of_a_newer_release() {
        let release = |tag: &str, names: &[&str]| Release {
            tag_name: Some(tag.into()),
            assets: names
                .iter()
                .map(|name| Asset {
                    name: Some((*name).into()),
                    browser_download_url: Some(format!("https://example.invalid/{name}")),
                })
                .collect(),
        };
        let files = [
            "DauntlessRevivedLauncher-0.2.0-linux-amd64.deb",
            "DauntlessRevivedLauncher-0.2.0-linux-x86_64.rpm",
        ];
        let newer_release = release("launcher-v0.2.0", &files);
        assert_eq!(
            package_in(&newer_release, "0.1.39", PackageKind::Deb).map(|(_, v)| v),
            Some("0.2.0".into())
        );
        assert!(
            package_in(&newer_release, "0.1.39", PackageKind::Rpm)
                .unwrap()
                .0
                .ends_with(".rpm")
        );
        assert_eq!(package_in(&newer_release, "0.2.0", PackageKind::Deb), None, "not newer");
        assert_eq!(
            package_in(&release("v0.3.0", &files), "0.1.39", PackageKind::Deb),
            None,
            "not a launcher tag"
        );
        assert_eq!(
            package_in(&release("launcher-v0.3.0", &files), "0.1.39", PackageKind::Deb),
            None,
            "no package of that version"
        );
    }
}
