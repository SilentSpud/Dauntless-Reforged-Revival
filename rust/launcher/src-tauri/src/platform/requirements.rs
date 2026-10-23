//! The Host page's Requirements tab (Reforged's `src/main/requirements.ts`): what hosting a server and
//! building its runtime need on this PC, each detected here, with what is wrong when something is
//! installed but will not work. The controller keeps the answer for 15 s.
//!
//! The planned layout (RD23): `reforged-host` comes with the workspace, so neither Node.js nor git is a
//! requirement; the database is MongoDB until the backend's cutover (RB14) and Postgres after it
//! (RD15); the C++ toolchain and cargo stay, for the runtime, CatalogExporter and the loader.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::bridge::types::{Requirement, RequirementId, RequirementState};
use crate::platform::hostkit::{Toolchain, exec_text};
use crate::platform::launch::system_root;

/// The server's database (RD15). RB14 switches it, with a `postgres` requirement id in the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Database {
    MongoDb,
    Postgres,
}

pub const DATABASE: Database = Database::MongoDb;

/// The oldest Postgres the backend runs on (RD15).
pub const POSTGRES_MINIMUM: u32 = 17;

/// What the checks look at, so the tests can give their own.
pub struct Probes<'a> {
    pub env: &'a (dyn Fn(&str) -> Option<OsString> + Sync),
    pub exists: &'a (dyn Fn(&Path) -> bool + Sync),
    pub list_dir: &'a (dyn Fn(&Path) -> Vec<String> + Sync),
}

pub fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        },
    )
}

impl Probes<'static> {
    pub fn system() -> Self {
        Self {
            env: &crate::platform::hostkit::system_env,
            exists: &|path: &Path| path.is_file(),
            list_dir: &list_dir,
        }
    }
}

fn path_dirs(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let Some(path) = env("PATH").or_else(|| env("Path")) else {
        return Vec::new();
    };
    path.to_string_lossy()
        .split(';')
        .map(|d| d.trim().trim_matches('"'))
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Folder names that are versions, newest first ("10.0" after "9.4"; anything else last).
fn newest_first(mut names: Vec<String>) -> Vec<String> {
    let key = |name: &str| -> Vec<u64> { name.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    names.sort_by_key(|name| std::cmp::Reverse(key(name)));
    names
}

/// `mongod.exe` on PATH, or in the newest version of the installer's folder (as the host kit's
/// `findMongod`).
pub fn find_mongod(p: &Probes<'_>) -> Option<PathBuf> {
    if let Some(found) = path_dirs(p.env)
        .into_iter()
        .map(|d| d.join("mongod.exe"))
        .find(|f| (p.exists)(f))
    {
        return Some(found);
    }
    let base = (p.env)("ProgramFiles")
        .map_or_else(|| PathBuf::from("C:\\Program Files"), PathBuf::from)
        .join("MongoDB")
        .join("Server");
    newest_first((p.list_dir)(&base))
        .into_iter()
        .map(|v| base.join(v).join("bin").join("mongod.exe"))
        .find(|f| (p.exists)(f))
}

/// The version folder in `…\MongoDB\Server\<version>\bin\mongod.exe`.
fn mongod_version(mongod: &Path) -> Option<String> {
    let parts: Vec<String> = mongod.iter().map(|c| c.to_string_lossy().into_owned()).collect();
    let at = parts.iter().position(|c| c.eq_ignore_ascii_case("Server"))?;
    let before = parts.get(at.checked_sub(1)?)?;
    before
        .eq_ignore_ascii_case("MongoDB")
        .then(|| parts.get(at + 1).cloned())
        .flatten()
}

/// Postgres's `pg_ctl.exe` on PATH, or in the newest version of the installer's folder, with that
/// version (`C:\Program Files\PostgreSQL\17\bin`).
pub fn find_postgres(p: &Probes<'_>) -> Option<(PathBuf, Option<u32>)> {
    if let Some(found) = path_dirs(p.env)
        .into_iter()
        .map(|d| d.join("pg_ctl.exe"))
        .find(|f| (p.exists)(f))
    {
        let version = found
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|v| v.to_string_lossy().parse().ok());
        return Some((found, version));
    }
    let base = (p.env)("ProgramFiles")
        .map_or_else(|| PathBuf::from("C:\\Program Files"), PathBuf::from)
        .join("PostgreSQL");
    newest_first((p.list_dir)(&base)).into_iter().find_map(|v| {
        let found = base.join(&v).join("bin").join("pg_ctl.exe");
        (p.exists)(&found).then(|| (found, v.split('.').next().and_then(|m| m.parse().ok())))
    })
}

/// What the controller knows that the checks need.
pub struct Inputs<'a> {
    /// The game folder is set, found and the right version; its version text
    pub game: (bool, Option<String>),
    pub toolchain: &'a Toolchain,
    pub dumper_dll: Option<&'a Path>,
    pub tailscale: bool,
    /// The Visual C++ runtime files the server DLL needs that Windows lacks
    pub missing_vc_runtime: usize,
}

fn req(id: RequirementId, state: RequirementState, detail: Option<String>, problem: Option<&str>) -> Requirement {
    Requirement {
        id,
        state,
        detail,
        problem: problem.map(str::to_owned),
    }
}

fn ok_or_missing(found: bool) -> RequirementState {
    if found {
        RequirementState::Ok
    } else {
        RequirementState::Missing
    }
}

/// The installer's default MongoDB Windows service holds port 27017 without a replica set.
async fn mongodb_service_running() -> bool {
    let sc = system_root().join("System32").join("sc.exe");
    exec_text(&sc, &["query", "MongoDB"])
        .await
        .is_some_and(|out| out.to_uppercase().contains("RUNNING"))
}

/// The requirements in the tab's order. `any_visual_studio` answers when the toolchain has no Visual
/// Studio with the C++ workload: one without it is a problem, not missing.
pub async fn check_requirements(
    inputs: &Inputs<'_>,
    probes: &Probes<'_>,
    service_running: impl AsyncFnOnce() -> bool,
    any_visual_studio: impl AsyncFnOnce() -> Option<String>,
) -> Vec<Requirement> {
    use RequirementId as Id;
    use RequirementState as State;
    let windows = cfg!(windows);
    let mut out = Vec::new();

    out.push(req(
        Id::Dauntless,
        ok_or_missing(inputs.game.0),
        inputs.game.1.clone(),
        None,
    ));

    match DATABASE {
        Database::MongoDb => match find_mongod(probes).filter(|_| windows) {
            None => out.push(req(Id::Mongodb, State::Missing, None, None)),
            Some(mongod) => {
                let version = mongod_version(&mongod);
                if service_running().await {
                    out.push(req(Id::Mongodb, State::Problem, version, Some("service_running")));
                } else {
                    out.push(req(Id::Mongodb, State::Ok, version, None));
                }
            }
        },
        // RB14: a `postgres` requirement, `too_old` below POSTGRES_MINIMUM (the id is the bridge's).
        Database::Postgres => {}
    }

    out.push(req(Id::Tailscale, ok_or_missing(inputs.tailscale), None, None));

    match &inputs.toolchain.visual_studio {
        Some(vs) => out.push(req(Id::VisualStudio, State::Ok, Some(vs.clone()), None)),
        None => match any_visual_studio().await {
            Some(vs) => out.push(req(Id::VisualStudio, State::Problem, Some(vs), Some("no_cpp"))),
            None => out.push(req(Id::VisualStudio, State::Missing, None, None)),
        },
    }

    out.push(req(
        Id::Rust,
        ok_or_missing(inputs.toolchain.cargo.is_some()),
        None,
        None,
    ));

    match inputs.dumper_dll.filter(|dll| (probes.exists)(dll)) {
        Some(dll) => out.push(req(
            Id::Dumper7,
            State::Ok,
            dll.file_name().map(|n| n.to_string_lossy().into_owned()),
            None,
        )),
        None => out.push(req(Id::Dumper7, State::Missing, None, None)),
    }

    out.push(req(
        Id::VcRedist,
        ok_or_missing(inputs.missing_vc_runtime == 0),
        None,
        None,
    ));
    out
}

/// The tab's list on this PC.
pub async fn check_requirements_here(inputs: &Inputs<'_>) -> Vec<Requirement> {
    check_requirements(
        inputs,
        &Probes::system(),
        mongodb_service_running,
        crate::platform::hostkit::any_visual_studio,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Probes over a fixed set of files.
    macro_rules! probes {
        ($p:ident, $env:expr, $files:expr, $dirs:expr) => {
            let files: &HashSet<PathBuf> = $files;
            let exists = |path: &Path| files.contains(path);
            let $p = Probes {
                env: $env,
                exists: &exists,
                list_dir: $dirs,
            };
        };
    }

    fn windows_env(name: &str) -> Option<OsString> {
        match name {
            "PATH" => Some("C:\\Windows\\System32".into()),
            "ProgramFiles" => Some("C:\\Program Files".into()),
            _ => None,
        }
    }

    // hostserver.test.ts: "Node.js, Git and MongoDB are found where Windows installs them" (MongoDB; the
    // planned layout needs neither Node.js nor git)
    #[test]
    fn mongodb_is_found_where_windows_installs_it() {
        let files: HashSet<PathBuf> = [PathBuf::from("C:\\Program Files")
            .join("MongoDB")
            .join("Server")
            .join("8.0")
            .join("bin")
            .join("mongod.exe")]
        .into();
        let dirs = |_: &Path| vec!["7.0".to_owned(), "8.0".to_owned()];
        probes!(p, &windows_env, &files, &dirs);
        let found = find_mongod(&p).unwrap();
        assert_eq!(
            found,
            PathBuf::from("C:\\Program Files")
                .join("MongoDB")
                .join("Server")
                .join("8.0")
                .join("bin")
                .join("mongod.exe")
        );
        assert_eq!(mongod_version(&found).as_deref(), Some("8.0"));
        let none = HashSet::new();
        probes!(empty, &|_| None, &none, &|_| Vec::new());
        assert_eq!(find_mongod(&empty), None);

        let on_path: HashSet<PathBuf> = [PathBuf::from("D:\\mongo\\bin").join("mongod.exe")].into();
        let env = |name: &str| (name == "Path").then(|| OsString::from("\"D:\\mongo\\bin\";C:\\x"));
        probes!(path, &env, &on_path, &|_| Vec::new());
        assert_eq!(
            find_mongod(&path),
            Some(PathBuf::from("D:\\mongo\\bin").join("mongod.exe"))
        );
    }

    #[test]
    fn postgres_is_found_with_its_version() {
        let base = PathBuf::from("C:\\Program Files").join("PostgreSQL");
        let files: HashSet<PathBuf> = [
            base.join("17").join("bin").join("pg_ctl.exe"),
            base.join("9.6").join("bin").join("pg_ctl.exe"),
        ]
        .into();
        let dirs = |_: &Path| vec!["9.6".to_owned(), "17".to_owned()];
        probes!(p, &windows_env, &files, &dirs);
        let (found, version) = find_postgres(&p).unwrap();
        assert_eq!(found, base.join("17").join("bin").join("pg_ctl.exe"));
        assert_eq!(version, Some(17));
        assert!(version.unwrap() >= POSTGRES_MINIMUM);
    }

    #[test]
    fn versions_sort_numerically() {
        assert_eq!(
            newest_first(vec!["9.4".into(), "10.0".into(), "8.0".into()]),
            ["10.0", "9.4", "8.0"]
        );
    }

    #[tokio::test]
    async fn the_list_has_no_node_or_git_and_says_what_is_wrong() {
        let dll = PathBuf::from("C:\\Tools").join("Dumper-7.dll");
        let mongod = PathBuf::from("C:\\Program Files")
            .join("MongoDB")
            .join("Server")
            .join("8.0")
            .join("bin")
            .join("mongod.exe");
        let files: HashSet<PathBuf> = [dll.clone(), mongod].into();
        let dirs = |_: &Path| vec!["8.0".to_owned()];
        probes!(p, &windows_env, &files, &dirs);
        let toolchain = Toolchain {
            cargo: Some(PathBuf::from("cargo.exe")),
            ..Toolchain::default()
        };
        let inputs = Inputs {
            game: (true, Some("1.14.7".into())),
            toolchain: &toolchain,
            dumper_dll: Some(&dll),
            tailscale: false,
            missing_vc_runtime: 1,
        };
        let list = check_requirements(
            &inputs,
            &p,
            async || true,
            async || Some("Visual Studio Community 2022".into()),
        )
        .await;
        let ids: Vec<RequirementId> = list.iter().map(|r| r.id).collect();
        let mut expected = vec![RequirementId::Dauntless];
        if cfg!(windows) {
            assert_eq!(list[1].problem.as_deref(), Some("service_running"));
            assert_eq!(list[1].detail.as_deref(), Some("8.0"));
        }
        expected.extend([
            RequirementId::Mongodb,
            RequirementId::Tailscale,
            RequirementId::VisualStudio,
            RequirementId::Rust,
            RequirementId::Dumper7,
            RequirementId::VcRedist,
        ]);
        assert_eq!(ids, expected);
        let get = |id| list.iter().find(|r| r.id == id).unwrap();
        assert_eq!(get(RequirementId::Dauntless).state, RequirementState::Ok);
        assert_eq!(get(RequirementId::Tailscale).state, RequirementState::Missing);
        assert_eq!(get(RequirementId::VisualStudio).problem.as_deref(), Some("no_cpp"));
        assert_eq!(get(RequirementId::Rust).state, RequirementState::Ok);
        assert_eq!(get(RequirementId::Dumper7).detail.as_deref(), Some("Dumper-7.dll"));
        assert_eq!(get(RequirementId::VcRedist).state, RequirementState::Missing);
    }
}
