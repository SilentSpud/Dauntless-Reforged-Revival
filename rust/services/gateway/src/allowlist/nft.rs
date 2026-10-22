//! Linux: the helper owns one nftables table and nothing else (`src/allowlist/nft.ts`). Its input
//! chain lets the game ports' UDP traffic through only from the addresses in its two sets and drops
//! the rest of it; loopback is never filtered. Every change is one `nft -f` transaction that swaps
//! the whole table, so there is never a moment with the ports open to everyone or the table missing.
//! nft runs at a fixed path with a fixed argument list: never PATH, never a shell (the helper has
//! `CAP_NET_ADMIN`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::json;
use service_kit::linelog;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::firewall::{RulePlan, plan};

/// The table a Debian host running the Node helper already has; the Rust helper replaces it.
pub const NFT_TABLE: &str = "reforged_revival";

/// Where Debian's `nftables` package puts nft, in the order they are tried.
const NFT_CANDIDATES: [&str; 2] = ["/usr/sbin/nft", "/sbin/nft"];

const RUN_TIMEOUT: Duration = Duration::from_secs(30);
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// nft's stderr is kept to its last this many bytes; an error shows the first `SHOWN` of those.
const STDERR_KEPT: usize = 4000;
const STDERR_SHOWN: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NftSettings {
    /// `ALLOWLIST_NFT` (absolute), else the first of [`NFT_CANDIDATES`] that exists
    pub binary: PathBuf,
    /// `ALLOWLIST_NFT_TABLE`, default [`NFT_TABLE`]
    pub table: String,
    /// `ALLOWLIST_INTERFACE`: only traffic arriving there is filtered. Unset: every interface but lo
    pub interface: Option<String>,
}

impl Default for NftSettings {
    fn default() -> Self {
        Self {
            binary: default_nft(),
            table: NFT_TABLE.into(),
            interface: None,
        }
    }
}

/// `CheckNftTable`: a letter, then up to 31 letters, digits or `_`.
pub fn check_nft_table(name: &str) -> Result<String, String> {
    let mut bytes = name.bytes();
    let valid = bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && name.len() <= 32
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !valid {
        return Err(format!(
            "ALLOWLIST_NFT_TABLE must be a letter then up to 31 letters, digits or _ (got {})",
            json!(name)
        ));
    }
    Ok(name.to_owned())
}

/// `CheckInterface`: a Linux interface name, at most 15 characters, no spaces, quotes or slashes.
pub fn check_interface(name: &str) -> Result<String, String> {
    let mut bytes = name.bytes();
    let valid = bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && name.len() <= 15
        && bytes.all(|b| b.is_ascii_alphanumeric() || b"_.@-".contains(&b));
    if !valid {
        return Err(format!(
            "ALLOWLIST_INTERFACE must be a network interface name like eth0 (got {})",
            json!(name)
        ));
    }
    Ok(name.to_owned())
}

/// `ALLOWLIST_NFT`: an absolute path (POSIX: it starts with `/`), so PATH is never searched.
pub fn check_nft_binary(path: &str) -> Result<PathBuf, String> {
    if !path.starts_with('/') {
        return Err("ALLOWLIST_NFT must be the absolute path of nft (the helper never looks it up in PATH)".into());
    }
    Ok(PathBuf::from(path))
}

/// `DefaultNft`: the first fixed path that exists, else `/usr/sbin/nft`. Never PATH.
pub fn default_nft() -> PathBuf {
    NFT_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.exists())
        .unwrap_or_else(|| PathBuf::from(NFT_CANDIDATES[0]))
}

/// `BuildNftScript`: the whole nftables script for one desired state. nft applies a file as one
/// transaction: the first line makes sure the table exists, the second deletes it, and the rest
/// defines it again with exactly these addresses.
///  - no addresses: the sets are empty and the game ports drop every packet (closed)
///  - addresses: only they reach the game ports
///
/// Addresses, ports, table and interface are checked again here, so no caller can ever put anything
/// else into a script.
pub fn nft_script(rule: &RulePlan, settings: &NftSettings) -> Result<String, String> {
    let rule = plan(&rule.ips, &rule.ports)?;
    let table = check_nft_table(&settings.table)?;
    let iif = match &settings.interface {
        Some(name) => format!("iifname \"{}\" ", check_interface(name)?),
        None => String::new(),
    };
    let (v4, v6): (Vec<&str>, Vec<&str>) = rule.ips.iter().map(String::as_str).partition(|ip| !ip.contains(':'));
    let set = |name: &str, kind: &str, elements: &[&str]| {
        let mut lines = vec![format!("\tset {name} {{"), format!("\t\ttype {kind}")];
        if !elements.is_empty() {
            lines.push(format!("\t\telements = {{ {} }}", elements.join(", ")));
        }
        lines.push("\t}".into());
        lines
    };
    let ports = &rule.ports;
    let mut lines = vec![
        format!("table inet {table} {{}}"),
        format!("delete table inet {table}"),
        format!("table inet {table} {{"),
    ];
    lines.extend(set("players4", "ipv4_addr", &v4));
    lines.extend(set("players6", "ipv6_addr", &v6));
    lines.extend([
        "\tchain input {".into(),
        "\t\ttype filter hook input priority -5; policy accept;".into(),
        "\t\tiifname \"lo\" accept".into(),
        format!("\t\t{iif}ip saddr @players4 udp dport {ports} accept"),
        format!("\t\t{iif}ip6 saddr @players6 udp dport {ports} accept"),
        format!("\t\t{iif}udp dport {ports} drop"),
        "\t}".into(),
        "}".into(),
    ]);
    Ok(lines.join("\n") + "\n")
}

/// How nft gets the script. Debian 13's nft is expected to read `-f -`; the Node helper's comment
/// says some nft refuses a pipe, so if stdin fails the first time and a file works, files are used
/// from then on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    Unknown,
    Stdin,
    File,
}

pub struct Nft {
    settings: NftSettings,
    feed: Mutex<Feed>,
    timeout: Duration,
}

impl Nft {
    pub fn new(settings: NftSettings) -> Self {
        Self {
            settings,
            feed: Mutex::new(Feed::Unknown),
            timeout: RUN_TIMEOUT,
        }
    }

    /// For tests: a shorter limit on one nft run.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn settings(&self) -> &NftSettings {
        &self.settings
    }

    /// How the last successful run fed the script.
    pub fn feed(&self) -> Feed {
        *self.feed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub async fn apply(&self, rule: &RulePlan) -> Result<(), String> {
        let script = nft_script(rule, &self.settings)?;
        self.run(&script).await
    }

    /// Runs one script: on stdin, or as a private file once stdin has been seen to fail.
    pub async fn run(&self, script: &str) -> Result<(), String> {
        let binary = &self.settings.binary;
        match self.feed() {
            Feed::Stdin => run_stdin(binary, script, self.timeout).await,
            Feed::File => run_file(binary, script, self.timeout).await,
            Feed::Unknown => {
                let refused = match run_stdin(binary, script, self.timeout).await {
                    Ok(()) => {
                        self.settle(Feed::Stdin);
                        return Ok(());
                    }
                    Err(error) => error,
                };
                run_file(binary, script, self.timeout).await?;
                linelog::warn(
                    "nft refused the script on stdin; using a private file from now on",
                    json!({ "detail": refused }),
                );
                self.settle(Feed::File);
                Ok(())
            }
        }
    }

    fn settle(&self, feed: Feed) {
        *self.feed.lock().unwrap_or_else(PoisonError::into_inner) = feed;
    }
}

/// The first and last part of nft's stderr, as `NftRunner` shows it.
fn failure(code: Option<i32>, stderr: &[u8]) -> String {
    let kept = &stderr[stderr.len().saturating_sub(STDERR_KEPT)..];
    let text = String::from_utf8_lossy(kept);
    let detail: String = text.trim().chars().take(STDERR_SHOWN).collect();
    let code = code.map_or_else(|| "a signal".to_owned(), |c| c.to_string());
    if detail.is_empty() {
        format!("nft exited with {code}")
    } else {
        format!("nft exited with {code}: {detail}")
    }
}

async fn run_nft(binary: &Path, args: &[&std::ffi::OsStr], input: Option<&str>, limit: Duration) -> Result<(), String> {
    let mut child = tokio::process::Command::new(binary)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", binary.display()))?;
    let stdin = child.stdin.take();
    let mut stderr = child.stderr.take();
    let run = async {
        let write = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                // A broken pipe is nft not reading stdin: its exit code says what happened
                let _ = stdin.write_all(input.as_bytes()).await;
            }
        };
        let read = async {
            let mut bytes = Vec::new();
            if let Some(stderr) = stderr.as_mut() {
                let _ = stderr.read_to_end(&mut bytes).await;
            }
            bytes
        };
        let ((), bytes, status) = tokio::join!(write, read, child.wait());
        (bytes, status)
    };
    match tokio::time::timeout(limit, run).await {
        Err(_) => Err(format!("nft did not finish within {} ms", limit.as_millis())),
        Ok((_, Err(error))) => Err(error.to_string()),
        Ok((_, Ok(status))) if status.success() => Ok(()),
        Ok((bytes, Ok(status))) => Err(failure(status.code(), &bytes)),
    }
}

async fn run_stdin(binary: &Path, script: &str, limit: Duration) -> Result<(), String> {
    run_nft(binary, &["-f".as_ref(), "-".as_ref()], Some(script), limit).await
}

/// The script as `rules.nft`, mode 0600, in a new folder of mode 0700; both are removed afterwards.
async fn run_file(binary: &Path, script: &str, limit: Duration) -> Result<(), String> {
    let (dir, file) = private_script(script).map_err(|e| format!("could not write the nft script: {e}"))?;
    let result = run_nft(binary, &["-f".as_ref(), file.as_os_str()], None, limit).await;
    drop(dir);
    result
}

pub fn private_script(script: &str) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("reforged-nft-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let dir = builder.tempdir()?;
    let file = dir.path().join("rules.nft");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&file)?.write_all(script.as_bytes())?;
    Ok((dir, file))
}

/// `CanManageNft`, read-only: can this process change nftables (root, or `CAP_NET_ADMIN`)? Real mode
/// refuses to start without it, because every firewall change would fail.
pub async fn can_manage_nft(binary: &Path) -> bool {
    if !binary.is_file() {
        return false;
    }
    run_nft(binary, &["list".as_ref(), "tables".as_ref()], None, CHECK_TIMEOUT)
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn settings(interface: Option<&str>) -> NftSettings {
        NftSettings {
            binary: PathBuf::from("/usr/sbin/nft"),
            table: NFT_TABLE.into(),
            interface: interface.map(str::to_owned),
        }
    }

    fn script(ips: &[&str], ports: &str, interface: Option<&str>) -> Result<String, String> {
        nft_script(
            &RulePlan {
                ips: strings(ips),
                ports: ports.into(),
            },
            &settings(interface),
        )
    }

    pub const EMPTY: &str = "table inet reforged_revival {}\n\
        delete table inet reforged_revival\n\
        table inet reforged_revival {\n\
        \tset players4 {\n\
        \t\ttype ipv4_addr\n\
        \t}\n\
        \tset players6 {\n\
        \t\ttype ipv6_addr\n\
        \t}\n\
        \tchain input {\n\
        \t\ttype filter hook input priority -5; policy accept;\n\
        \t\tiifname \"lo\" accept\n\
        \t\tip saddr @players4 udp dport 8770-8777 accept\n\
        \t\tip6 saddr @players6 udp dport 8770-8777 accept\n\
        \t\tudp dport 8770-8777 drop\n\
        \t}\n\
        }\n";

    const TWO_ON_ETH0: &str = "table inet reforged_revival {}\n\
        delete table inet reforged_revival\n\
        table inet reforged_revival {\n\
        \tset players4 {\n\
        \t\ttype ipv4_addr\n\
        \t\telements = { 203.0.113.7, 198.51.100.2 }\n\
        \t}\n\
        \tset players6 {\n\
        \t\ttype ipv6_addr\n\
        \t\telements = { 2001:db8::1 }\n\
        \t}\n\
        \tchain input {\n\
        \t\ttype filter hook input priority -5; policy accept;\n\
        \t\tiifname \"lo\" accept\n\
        \t\tiifname \"eth0\" ip saddr @players4 udp dport 8770-8777 accept\n\
        \t\tiifname \"eth0\" ip6 saddr @players6 udp dport 8770-8777 accept\n\
        \t\tiifname \"eth0\" udp dport 8770-8777 drop\n\
        \t}\n\
        }\n";

    #[test]
    fn is_exactly_this_for_an_empty_set_the_game_ports_drop_everything() {
        assert_eq!(script(&[], "8770-8777", None).unwrap(), EMPTY);
    }

    #[test]
    fn is_exactly_this_for_three_addresses_on_one_interface() {
        assert_eq!(
            script(
                &["203.0.113.7", "2001:db8::1", "198.51.100.2"],
                "8770-8777",
                Some("eth0")
            )
            .unwrap(),
            TWO_ON_ETH0
        );
    }

    #[test]
    fn a_single_port_stays_a_single_port() {
        let text = script(&["203.0.113.7"], "8777", None).unwrap();
        assert!(text.contains("\t\tip saddr @players4 udp dport 8777 accept\n"));
        assert!(text.contains("\t\tudp dport 8777 drop\n"));
    }

    #[test]
    fn refuses_anything_but_canonical_addresses_a_port_range_a_plain_table_and_interface() {
        for bad in [
            "2001:DB8::1",
            "1.2.3.4/32",
            "0.0.0.0",
            "any",
            "1.2.3.4 } ; flush ruleset ; {",
            "",
            "010.1.1.1",
            "224.0.0.1",
        ] {
            assert!(
                script(&[bad], "8770-8777", None).unwrap_err().contains("refusing"),
                "{bad}"
            );
        }
        assert!(
            script(&["203.0.113.7"], "8770-8777 accept", None)
                .unwrap_err()
                .contains("ALLOWLIST_PORTS")
        );
        let mut bad_table = settings(None);
        bad_table.table = "t; flush ruleset".into();
        let empty = RulePlan {
            ips: Vec::new(),
            ports: "8770-8777".into(),
        };
        assert!(
            nft_script(&empty, &bad_table)
                .unwrap_err()
                .contains("ALLOWLIST_NFT_TABLE")
        );
        assert!(
            script(&[], "8770-8777", Some("eth0\" accept"))
                .unwrap_err()
                .contains("ALLOWLIST_INTERFACE")
        );
    }

    #[test]
    fn table_interface_and_binary_names() {
        for good in ["reforged_revival", "a", "my_table", &format!("a{}", "b".repeat(31))] {
            assert_eq!(check_nft_table(good).unwrap(), good);
        }
        for bad in ["1table", "my-table", "t; flush ruleset", &"a".repeat(33), "", "_t"] {
            assert!(check_nft_table(bad).is_err(), "{bad}");
        }
        for good in ["eth0", "ens3", "wg0", "br-lan", "eth0.100", "0abc", &"x".repeat(15)] {
            assert_eq!(check_interface(good).unwrap(), good);
        }
        for bad in ["eth0\" accept", "a b", &"x".repeat(16), "../x", "", "-eth0", "eth0:1"] {
            assert!(check_interface(bad).is_err(), "{bad}");
        }
        assert!(check_nft_binary("/usr/sbin/nft").is_ok());
        for bad in ["nft", "./nft", "usr/sbin/nft", "C:\\nft.exe", ""] {
            assert!(check_nft_binary(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn finds_nft_at_a_fixed_path() {
        let found = default_nft();
        assert!(NFT_CANDIDATES.iter().any(|c| found == Path::new(c)), "{found:?}");
    }

    #[test]
    fn stderr_is_trimmed_to_its_first_part() {
        assert_eq!(failure(Some(1), b""), "nft exited with 1");
        assert_eq!(
            failure(Some(1), b"  Error: syntax\n"),
            "nft exited with 1: Error: syntax"
        );
        let long = "x".repeat(10_000);
        assert_eq!(
            failure(None, long.as_bytes()).len(),
            "nft exited with a signal: ".len() + 1000
        );
    }

    #[test]
    fn the_private_script_file() {
        let (dir, file) = private_script("table inet t {}\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "table inet t {}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!((mode(dir.path()), mode(&file)), (0o700, 0o600));
        }
        let path = dir.path().to_owned();
        drop(dir);
        assert!(!path.exists());
    }

    /// A stand-in for nft: a shell script that records how it got the script.
    #[cfg(unix)]
    mod with_a_fake_nft {
        use super::*;

        fn fake(dir: &Path, body: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let file = dir.join("nft");
            std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            file
        }

        fn nft(binary: PathBuf) -> Nft {
            Nft::new(NftSettings {
                binary,
                table: NFT_TABLE.into(),
                interface: None,
            })
            .with_timeout(Duration::from_secs(5))
        }

        fn closed() -> RulePlan {
            RulePlan {
                ips: Vec::new(),
                ports: "8770-8777".into(),
            }
        }

        #[tokio::test]
        async fn stdin_first() {
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("got");
            let binary = fake(
                dir.path(),
                &format!("[ \"$1 $2\" = \"-f -\" ] || exit 9\ncat > '{}'", out.display()),
            );
            let nft = nft(binary);
            nft.apply(&closed()).await.unwrap();
            assert_eq!(nft.feed(), Feed::Stdin);
            assert_eq!(std::fs::read_to_string(&out).unwrap(), EMPTY);
        }

        #[tokio::test]
        async fn a_private_file_when_stdin_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("got");
            let modes = dir.path().join("modes");
            let binary = fake(
                dir.path(),
                &format!(
                    "[ \"$2\" = \"-\" ] && {{ echo 'Error: stdin is not a file' >&2; exit 1; }}\n\
                     stat -c '%a' \"$(dirname \"$2\")\" \"$2\" | tr '\\n' ' ' > '{}'\n\
                     echo \"$2\" >> '{}'\n\
                     cat \"$2\" > '{}'",
                    modes.display(),
                    modes.display(),
                    out.display()
                ),
            );
            let nft = nft(binary);
            nft.apply(&closed()).await.unwrap();
            assert_eq!(nft.feed(), Feed::File);
            assert_eq!(std::fs::read_to_string(&out).unwrap(), EMPTY);
            let seen = std::fs::read_to_string(&modes).unwrap();
            let mut parts = seen.split_whitespace();
            assert_eq!((parts.next(), parts.next()), (Some("700"), Some("600")), "{seen}");
            let file = PathBuf::from(parts.next().unwrap());
            assert!(file.ends_with("rules.nft"));
            assert!(
                !file.exists() && !file.parent().unwrap().exists(),
                "the script is removed"
            );

            // Once files work, stdin is not tried again
            std::fs::remove_file(&out).unwrap();
            nft.apply(&closed()).await.unwrap();
            assert_eq!(std::fs::read_to_string(&out).unwrap(), EMPTY);
        }

        #[tokio::test]
        async fn failures_and_timeouts() {
            let dir = tempfile::tempdir().unwrap();
            let failing = nft(fake(dir.path(), "echo 'Error: syntax error' >&2\nexit 1"));
            let error = failing.apply(&closed()).await.unwrap_err();
            assert_eq!(error, "nft exited with 1: Error: syntax error");
            assert_eq!(failing.feed(), Feed::Unknown);

            let slow_dir = tempfile::tempdir().unwrap();
            let slow = nft(fake(slow_dir.path(), "sleep 10")).with_timeout(Duration::from_millis(300));
            assert!(slow.apply(&closed()).await.unwrap_err().contains("did not finish"));

            let missing = nft(dir.path().join("nothing"));
            assert!(missing.apply(&closed()).await.unwrap_err().contains("could not run"));
            assert!(!can_manage_nft(&dir.path().join("nothing")).await);
            assert!(can_manage_nft(&fake(slow_dir.path(), "exit 0")).await);
        }
    }

    /// Opt-in (`REFORGED_TEST_NFT=1`, Linux, root or `CAP_NET_ADMIN`): really applies the scripts to a
    /// table of its own on ports nothing uses, and deletes the table afterwards.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn nftables_for_real() {
        if std::env::var("REFORGED_TEST_NFT").as_deref() != Ok("1") {
            eprintln!("set REFORGED_TEST_NFT=1 on Linux as root to run");
            return;
        }
        let binary = default_nft();
        assert!(can_manage_nft(&binary).await, "{} cannot list tables", binary.display());
        let table = "reforged_revival_test";
        let ports = "64770-64771";
        let nft = Nft::new(NftSettings {
            binary: binary.clone(),
            table: table.into(),
            interface: None,
        });
        let list = || {
            let out = std::process::Command::new(&binary)
                .args(["list", "table", "inet", table])
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap()
        };
        let nft = &nft;
        let apply = |ips: &[&str]| {
            let rule = RulePlan {
                ips: strings(ips),
                ports: ports.into(),
            };
            async move { nft.apply(&rule).await }
        };
        let result = async {
            apply(&["203.0.113.7", "2001:db8::1"]).await?;
            let out = list();
            assert!(out.contains("203.0.113.7") && out.contains("2001:db8::1"), "{out}");
            assert!(out.contains("udp dport 64770-64771 drop"), "{out}");
            assert!(out.contains("iifname \"lo\" accept"), "{out}");
            eprintln!("nft fed by {:?}", nft.feed());

            // A second apply replaces the first (the table already exists)
            apply(&["198.51.100.9"]).await?;
            let out = list();
            assert!(out.contains("198.51.100.9") && !out.contains("203.0.113.7"), "{out}");

            // Empty: still there, closed
            apply(&[]).await?;
            let out = list();
            assert!(
                !out.contains("elements") && out.contains("udp dport 64770-64771 drop"),
                "{out}"
            );

            assert!(
                nft.run("this is not nftables\n")
                    .await
                    .unwrap_err()
                    .contains("nft exited with")
            );
            Ok::<(), String>(())
        }
        .await;
        let _ = std::process::Command::new(&binary)
            .args(["delete", "table", "inet", table])
            .status();
        result.unwrap();
    }
}
