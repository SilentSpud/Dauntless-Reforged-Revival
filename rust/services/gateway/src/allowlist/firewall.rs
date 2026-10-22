//! What the helper does to the firewall, for one desired address set. On Windows: one Windows
//! Firewall rule through the COM API (`INetFwPolicy2`, no PowerShell); on Linux: one nftables table
//! fed to `nft -f -` with a fixed argument list (no shell). Dry run only describes the change in the
//! audit log. Addresses reach this point already validated; [`plan`] checks them again so nothing
//! else can ever reach a firewall.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};
use service_kit::env::Env;

use super::config::check_ports;
use crate::ip::{AllowlistCheck, check_allowlist_ip};

/// The one Windows Firewall rule this helper owns, by the name the host sees in "Windows Defender
/// Firewall with Advanced Security" (the Node helper's `DisplayName`; COM knows rules by it).
pub const RULE_DISPLAY_NAME: &str = "Dauntless Revived game ports (allowlist)";

/// One desired state: open the game ports to exactly `ips`, or (none) close them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulePlan {
    pub ips: Vec<String>,
    pub ports: String,
}

impl RulePlan {
    pub fn enabled(&self) -> bool {
        !self.ips.is_empty()
    }

    fn low_high(&self) -> (u16, u16) {
        let (low, high) = self.ports.split_once('-').unwrap_or((&self.ports, &self.ports));
        (low.parse().unwrap_or(0), high.parse().unwrap_or(0))
    }
}

/// `BuildFirewallScript`'s checks: canonical single addresses and a port or range only.
pub fn plan(ips: &[String], ports: &str) -> Result<RulePlan, String> {
    let ports = check_ports(ports)?;
    for ip in ips {
        match check_allowlist_ip(Some(ip), true) {
            AllowlistCheck::Ok(parsed) if parsed.canonical == *ip => {}
            _ => return Err(format!("refusing to change the firewall with {}", json!(ip))),
        }
    }
    Ok(RulePlan {
        ips: ips.to_vec(),
        ports,
    })
}

// ---- Linux: nftables ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NftSettings {
    /// `DR_NFT_BINARY`, else `nft` on PATH, else /usr/sbin/nft or /sbin/nft
    pub binary: Option<PathBuf>,
    /// `DR_NFT_TABLE` (letters, digits, underscores), default `dauntless_revived`
    pub table: String,
    /// `DR_FIREWALL_INTERFACE`: only traffic arriving there is filtered
    pub interface: Option<String>,
}

impl NftSettings {
    pub fn from_env(env: &Env) -> Result<Self, String> {
        let table = env
            .raw("DR_NFT_TABLE")
            .filter(|t| !t.is_empty())
            .unwrap_or("dauntless_revived")
            .to_owned();
        if !(1..=32).contains(&table.len()) || !table.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err("DR_NFT_TABLE is invalid".into());
        }
        let interface = env
            .raw("DR_FIREWALL_INTERFACE")
            .filter(|i| !i.is_empty())
            .map(str::to_owned);
        if let Some(interface) = &interface
            && (interface.len() > 32
                || !interface
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:@-".contains(&b)))
        {
            return Err("DR_FIREWALL_INTERFACE is invalid".into());
        }
        Ok(Self {
            binary: env.raw("DR_NFT_BINARY").filter(|b| !b.is_empty()).map(PathBuf::from),
            table,
            interface,
        })
    }
}

/// The table, its two sets and the rules: game-port traffic from the sets passes, the rest of it
/// is dropped; everything else is left to the host's own rules.
pub fn nft_table_script(settings: &NftSettings, plan: &RulePlan) -> String {
    let table = &settings.table;
    let (low, high) = plan.low_high();
    let iif = settings
        .interface
        .as_ref()
        .map(|i| format!("iifname \"{i}\" "))
        .unwrap_or_default();
    format!(
        "add table inet {table}\n\
         add set inet {table} players4 {{ type ipv4_addr; }}\n\
         add set inet {table} players6 {{ type ipv6_addr; }}\n\
         add chain inet {table} input {{ type filter hook input priority -5; policy accept; }}\n\
         add rule inet {table} input {iif}ip saddr @players4 udp dport {low}-{high} accept\n\
         add rule inet {table} input {iif}ip6 saddr @players6 udp dport {low}-{high} accept\n\
         add rule inet {table} input {iif}udp dport {low}-{high} drop\n"
    )
}

/// The two sets' new contents.
pub fn nft_set_script(settings: &NftSettings, plan: &RulePlan) -> String {
    let table = &settings.table;
    let (v4, v6): (Vec<&String>, Vec<&String>) = plan.ips.iter().partition(|ip| !ip.contains(':'));
    let join = |list: &[&String]| list.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    let mut script = format!("flush set inet {table} players4\nflush set inet {table} players6\n");
    if !v4.is_empty() {
        script.push_str(&format!("add element inet {table} players4 {{ {} }}\n", join(&v4)));
    }
    if !v6.is_empty() {
        script.push_str(&format!("add element inet {table} players6 {{ {} }}\n", join(&v6)));
    }
    script
}

fn executable(file: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        file.is_file()
    }
}

fn find_nft(settings: &NftSettings) -> Option<PathBuf> {
    if let Some(explicit) = &settings.binary {
        return executable(explicit).then(|| explicit.clone());
    }
    let on_path = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("nft"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    on_path
        .into_iter()
        .chain(["/usr/sbin/nft", "/sbin/nft"].map(PathBuf::from))
        .find(|candidate| executable(candidate))
}

async fn run_nft(nft: &Path, script: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(nft)
        .args(["-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "nft failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub struct Nft {
    settings: NftSettings,
    /// The table settings last set up, so the table is only rebuilt when they change
    prepared: Mutex<Option<String>>,
}

impl Nft {
    pub fn new(settings: NftSettings) -> Self {
        Self {
            settings,
            prepared: Mutex::new(None),
        }
    }

    async fn apply(&self, plan: &RulePlan) -> Result<(), String> {
        let nft = find_nft(&self.settings).ok_or("nft was not found")?;
        let table = &self.settings.table;
        let key = format!(
            "{table}:{}:{}",
            self.settings.interface.as_deref().unwrap_or(""),
            plan.ports
        );
        let prepared = self.prepared.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if prepared.as_deref() != Some(key.as_str()) {
            let exists = tokio::process::Command::new(&nft)
                .args(["list", "table", "inet", table.as_str()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .is_ok_and(|s| s.success());
            if exists {
                run_nft(&nft, &format!("delete table inet {table}\n")).await?;
            }
            run_nft(&nft, &nft_table_script(&self.settings, plan)).await?;
            *self.prepared.lock().unwrap_or_else(PoisonError::into_inner) = Some(key);
        }
        run_nft(&nft, &nft_set_script(&self.settings, plan)).await
    }
}

// ---- the backends ----

pub enum Backend {
    /// Only writes what it would do to the audit log
    DryRun {
        describes: &'static str,
        nft: Option<NftSettings>,
    },
    #[cfg(windows)]
    WindowsFirewall,
    Nftables(Nft),
}

impl Backend {
    /// The real backend of this platform.
    pub fn for_platform(nft: NftSettings) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let _ = nft;
            Ok(Self::WindowsFirewall)
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self::Nftables(Nft::new(nft)))
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = nft;
            Err("ALLOWLIST_DRY_RUN=0 needs Windows (Windows Firewall) or Linux (nftables)".into())
        }
    }

    /// A dry run that describes what this platform's backend would do.
    pub fn dry_run(nft: NftSettings) -> Self {
        if cfg!(windows) {
            Self::DryRun {
                describes: "windows-firewall",
                nft: None,
            }
        } else {
            Self::DryRun {
                describes: "linux-nftables",
                nft: Some(nft),
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::DryRun { describes, .. } => describes,
            #[cfg(windows)]
            Self::WindowsFirewall => "windows-firewall",
            Self::Nftables(_) => "linux-nftables",
        }
    }

    /// What a dry run writes to the audit log for `plan`.
    pub fn describe(&self, plan: &RulePlan) -> Value {
        let mut out = json!({
            "rule": RULE_DISPLAY_NAME,
            "enabled": plan.enabled(),
            "ips": plan.ips,
            "ports": plan.ports,
        });
        if let Self::DryRun {
            nft: Some(settings), ..
        } = self
        {
            out["script"] = json!(nft_set_script(settings, plan));
        }
        out
    }

    /// Changes the firewall (never for a dry run, which the caller logs instead).
    pub async fn apply(&self, plan: &RulePlan) -> Result<(), String> {
        match self {
            Self::DryRun { .. } => Ok(()),
            #[cfg(windows)]
            Self::WindowsFirewall => {
                let plan = plan.clone();
                tokio::task::spawn_blocking(move || super::windows::apply_rule(&plan))
                    .await
                    .map_err(|e| e.to_string())?
            }
            Self::Nftables(nft) => nft.apply(plan).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn plans_only_canonical_single_addresses() {
        let plan = plan(&strings(&["203.0.113.7", "2001:db8::1"]), "8770-8777").unwrap();
        assert!(plan.enabled());
        assert!(!super::plan(&[], "8770-8777").unwrap().enabled());
        for bad in [
            "2001:DB8::1",
            "1.2.3.4/32",
            "0.0.0.0",
            "any",
            "1.2.3.4'; Remove-NetFirewallRule -All; '",
            "",
            "010.1.1.1",
            "224.0.0.1",
        ] {
            assert!(
                super::plan(&strings(&[bad]), "8770-8777")
                    .unwrap_err()
                    .contains("refusing"),
                "{bad}"
            );
        }
        assert!(
            super::plan(&strings(&["203.0.113.7"]), "8770-8777; Remove-NetFirewallRule")
                .unwrap_err()
                .contains("ALLOWLIST_PORTS")
        );
    }

    #[test]
    fn the_nft_scripts() {
        let settings = NftSettings {
            binary: None,
            table: "dauntless_revived".into(),
            interface: Some("eth0".into()),
        };
        let plan = plan(&strings(&["198.51.100.2", "203.0.113.7", "2001:db8::5"]), "8770-8777").unwrap();
        assert_eq!(
            nft_set_script(&settings, &plan),
            "flush set inet dauntless_revived players4\nflush set inet dauntless_revived players6\nadd element inet dauntless_revived players4 { 198.51.100.2, 203.0.113.7 }\nadd element inet dauntless_revived players6 { 2001:db8::5 }\n"
        );
        let table = nft_table_script(&settings, &plan);
        assert!(table.contains("add rule inet dauntless_revived input iifname \"eth0\" udp dport 8770-8777 drop\n"));
        assert!(table.contains("ip6 saddr @players6 udp dport 8770-8777 accept"));
        let closed = super::plan(&[], "8777").unwrap();
        assert_eq!(
            nft_set_script(&settings, &closed),
            "flush set inet dauntless_revived players4\nflush set inet dauntless_revived players6\n"
        );
        assert!(nft_table_script(&settings, &closed).contains("udp dport 8777-8777 drop"));
    }

    #[test]
    fn nft_settings_are_checked() {
        let env = |pairs: &[(&str, &str)]| NftSettings::from_env(&Env::from_pairs(pairs.iter().copied()));
        assert_eq!(env(&[]).unwrap().table, "dauntless_revived");
        assert!(env(&[("DR_NFT_TABLE", "bad table")]).is_err());
        assert!(env(&[("DR_FIREWALL_INTERFACE", "eth0\"; drop")]).is_err());
        assert_eq!(
            env(&[("DR_FIREWALL_INTERFACE", "wg0")]).unwrap().interface.as_deref(),
            Some("wg0")
        );
    }
}
