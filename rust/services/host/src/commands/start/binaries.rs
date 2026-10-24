//! Which program runs each service. The backend and the Director are Node until RB14. The gateway,
//! the content server and the allowlist helper move to Rust one row at a time: [`rust_ported`] is
//! the one switch each row flips. A ported service runs as `reforged-host <service>` (its cargo
//! feature, [`crate::servers`]) and falls back to its Node build in a binary built without it.

use std::path::{Path, PathBuf};

use crate::error::HostError;
use crate::paths::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontService {
    Gateway,
    Content,
    Allowlist,
}

/// Whether the service's Rust bin replaces its Node build (MIGRATION_PLAN.md section 4.3).
pub fn rust_ported(service: FrontService) -> bool {
    match service {
        // RS3 flips this.
        FrontService::Gateway => false,
        // RS2 flips this.
        FrontService::Content => false,
        // RS4: DR's allowlist with Reforged's port range and nftables variant.
        FrontService::Allowlist => true,
    }
}

impl FrontService {
    pub fn bin_name(self) -> &'static str {
        match self {
            Self::Gateway => "gateway",
            Self::Content => "content",
            Self::Allowlist => "allowlist",
        }
    }

    /// The Rust server's arguments after `reforged-host`: its name, then its env file with
    /// `--env-file` (DR X7).
    fn rust_args(self, env_file: &Path) -> Vec<String> {
        let env_file = env_file.display().to_string();
        let name = self.bin_name().to_owned();
        match self {
            // Content takes the file on its serve command too (`content --env-file` is refused).
            Self::Gateway | Self::Content => vec![name, "serve".into(), "--env-file".into(), env_file],
            Self::Allowlist => vec![name, "--env-file".into(), env_file],
        }
    }

    fn node_dir(self, paths: &Paths) -> PathBuf {
        match self {
            Self::Gateway | Self::Allowlist => paths.gateway_dir.clone(),
            Self::Content => paths.content_dir.clone(),
        }
    }

    fn node_script(self) -> PathBuf {
        match self {
            Self::Gateway | Self::Content => ["dist", "server.js"].iter().collect(),
            Self::Allowlist => ["dist", "allowlist", "server.js"].iter().collect(),
        }
    }
}

/// How to start one program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The env file is in `args` (a Rust bin); otherwise it is read into the environment.
    pub env_file_in_args: bool,
}

/// Where the programs are: Node, and this `reforged-host`, which runs the Rust servers built into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programs {
    pub node: PathBuf,
    /// `None` runs every service on Node.
    pub host: Option<PathBuf>,
}

impl Programs {
    /// Node from PATH (or where its Windows installer and nvm-windows put it), and this binary.
    pub fn find() -> Result<Self, HostError> {
        let node = find_node(&std::env::vars().collect::<Vec<_>>(), Path::exists).ok_or_else(|| {
            HostError::message(
                "Node.js (node) was not found: the backend and the Director still run on it. Install Node.js 22 or newer from https://nodejs.org",
            )
        })?;
        Ok(Self {
            node,
            host: std::env::current_exe().ok(),
        })
    }

    /// This binary, when `name` is built into it.
    pub fn server(&self, name: &str) -> Option<PathBuf> {
        self.host.clone().filter(|_| crate::servers::built_in(name))
    }

    /// The program for `service`, given its env file.
    pub fn launch(&self, service: FrontService, paths: &Paths, env_file: &Path) -> Launch {
        if rust_ported(service)
            && let Some(bin) = self.server(service.bin_name())
        {
            return Launch {
                command: bin,
                args: service.rust_args(env_file),
                cwd: paths.workspace.clone(),
                env_file_in_args: true,
            };
        }
        let dir = service.node_dir(paths);
        Launch {
            command: self.node.clone(),
            args: vec![service.node_script().display().to_string()],
            cwd: dir,
            env_file_in_args: false,
        }
    }
}

/// `node` on PATH, then in `%ProgramFiles%\nodejs` and nvm-windows' link (the launcher's `findNode`).
pub fn find_node(env: &[(String, String)], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let windows = cfg!(windows);
    let get = |name: &str| {
        env.iter()
            .find(|(k, _)| {
                if windows {
                    k.eq_ignore_ascii_case(name)
                } else {
                    k == name
                }
            })
            .map(|(_, v)| v.clone())
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = get("PATH") {
        for dir in std::env::split_paths(&path) {
            let text = dir.to_string_lossy();
            let text = text.trim().trim_matches('"');
            if !text.is_empty() {
                dirs.push(PathBuf::from(text));
            }
        }
    }
    if windows {
        for base in [
            get("ProgramW6432"),
            get("ProgramFiles"),
            Some("C:\\Program Files".into()),
        ]
        .into_iter()
        .flatten()
        {
            dirs.push(Path::new(&base).join("nodejs"));
        }
        if let Some(link) = get("NVM_SYMLINK") {
            dirs.push(PathBuf::from(link));
        }
    }
    let exe = if windows { "node.exe" } else { "node" };
    dirs.into_iter().map(|d| d.join(exe)).find(|f| exists(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programs(host: Option<&Path>) -> Programs {
        Programs {
            node: PathBuf::from("node"),
            host: host.map(Path::to_path_buf),
        }
    }

    #[test]
    fn runs_a_ported_service_as_this_binary_and_on_node_without_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let env_file = paths.config.join("allowlist.env");
        let p = programs(None);
        let node = p.launch(FrontService::Allowlist, &paths, &env_file);
        assert_eq!(node.command, PathBuf::from("node"));
        assert_eq!(
            node.args,
            vec![
                Path::new("dist")
                    .join("allowlist")
                    .join("server.js")
                    .display()
                    .to_string()
            ]
        );
        assert_eq!(node.cwd, paths.gateway_dir);
        assert!(!node.env_file_in_args);

        let bin = dir.path().join("reforged-host");
        let rust = programs(Some(&bin)).launch(FrontService::Allowlist, &paths, &env_file);
        assert_eq!(rust.command, bin);
        assert_eq!(rust.args, ["allowlist", "--env-file", &env_file.display().to_string()]);
        assert!(rust.env_file_in_args);
    }

    #[test]
    fn keeps_a_service_that_is_not_ported_yet_on_node_even_when_built_in() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let p = programs(Some(&dir.path().join("reforged-host")));
        assert!(!rust_ported(FrontService::Gateway) && !rust_ported(FrontService::Content));
        let gateway = p.launch(FrontService::Gateway, &paths, &paths.config.join("gateway.env"));
        assert_eq!(gateway.command, PathBuf::from("node"));
        assert_eq!(gateway.cwd, paths.gateway_dir);
        let content = p.launch(FrontService::Content, &paths, &paths.config.join("content.env"));
        assert_eq!(content.cwd, paths.content_dir);
        assert_eq!(
            content.args,
            vec![Path::new("dist").join("server.js").display().to_string()]
        );
    }

    #[test]
    fn gives_the_rust_gateway_and_content_their_serve_command() {
        let args = FrontService::Gateway.rust_args(Path::new("g.env"));
        assert_eq!(args, ["gateway", "serve", "--env-file", "g.env"]);
        let args = FrontService::Content.rust_args(Path::new("c.env"));
        assert_eq!(args, ["content", "serve", "--env-file", "c.env"]);
    }

    #[test]
    fn finds_node_on_path_then_in_the_usual_places() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let exe = if cfg!(windows) { "node.exe" } else { "node" };
        let env = vec![("PATH".to_string(), format!("/a{sep}\"/b\"{sep}"))];
        let found = find_node(&env, |p| p == Path::new("/b").join(exe));
        assert_eq!(found, Some(Path::new("/b").join(exe)));
        assert_eq!(find_node(&env, |_| false), None);
        if cfg!(windows) {
            let env = vec![("ProgramFiles".to_string(), "D:\\PF".to_string())];
            let want = Path::new("D:\\PF").join("nodejs").join(exe);
            assert_eq!(find_node(&env, |p| p == want), Some(want.clone()));
        }
    }
}
