//! What the helper does to the firewall, for one desired address set (`src/allowlist/firewall.ts`).
//! `ALLOWLIST_FIREWALL` picks it: `windows` is one Windows Firewall rule through the COM API
//! (`INetFwPolicy2`, no PowerShell), `nftables` is one nftables table swapped in a single `nft -f`
//! transaction ([`super::nft`]). Dry run only describes the change in the audit log. Addresses reach
//! this point already validated; [`plan`] checks them again so nothing else can ever reach a firewall.

use serde_json::{Value, json};

use super::config::{AllowlistConfig, FirewallKind};
use super::nft::{Nft, NftSettings, nft_script};
use super::ports::check_ports;
use crate::ip::{AllowlistCheck, check_allowlist_ip};

/// The one Windows Firewall rule this helper owns. `RULE_NAME` is the id the Node helper gave it;
/// COM knows rules by their display name, which is what the host sees in "Windows Defender Firewall
/// with Advanced Security". A rule the Node helper made has the same display name and is taken over.
pub const RULE_NAME: &str = "ReforgedRevival-GamePorts-Allowlist";
pub const RULE_DISPLAY_NAME: &str = "Dauntless Reforged Revival game ports (allowlist)";
pub const RULE_GROUPING: &str = "Dauntless Reforged Revival";
pub const RULE_DESCRIPTION: &str =
    "Opens the game ports to players who logged in through the gateway. Managed by the allowlist helper.";

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

// ---- Windows Firewall ----

/// The rule's settings while the ports are open: inbound UDP on the game ports, Allow, only from
/// the plan's addresses, every profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsRule {
    pub display_name: &'static str,
    pub description: &'static str,
    pub grouping: &'static str,
    pub direction: &'static str,
    pub action: &'static str,
    pub protocol: &'static str,
    pub local_ports: String,
    /// Comma-separated, as `INetFwRule::RemoteAddresses` takes them
    pub remote_addresses: String,
    pub profiles: &'static str,
}

/// What the helper does to its Windows Firewall rule for one plan. Any other rule with the same
/// display name is disabled either way, so exactly one is ever in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsChange {
    /// Create the rule or update it to these settings, and enable it
    Open(WindowsRule),
    /// Disable the rule (Windows then drops the game ports' traffic by default)
    Close,
}

pub fn windows_change(rule: &RulePlan) -> WindowsChange {
    if !rule.enabled() {
        return WindowsChange::Close;
    }
    WindowsChange::Open(WindowsRule {
        display_name: RULE_DISPLAY_NAME,
        description: RULE_DESCRIPTION,
        grouping: RULE_GROUPING,
        direction: "in",
        action: "allow",
        protocol: "udp",
        local_ports: rule.ports.clone(),
        remote_addresses: rule.ips.join(","),
        profiles: "all",
    })
}

impl WindowsChange {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Close => json!({ "enabled": false }),
            Self::Open(rule) => json!({
                "enabled": true,
                "displayName": rule.display_name,
                "direction": rule.direction,
                "action": rule.action,
                "protocol": rule.protocol,
                "localPorts": rule.local_ports,
                "remoteAddresses": rule.remote_addresses,
                "profiles": rule.profiles,
            }),
        }
    }
}

// ---- the backends ----

pub enum Backend {
    /// Only writes what it would do to the audit log
    DryRun {
        firewall: FirewallKind,
        nft: NftSettings,
    },
    #[cfg(windows)]
    WindowsFirewall,
    Nftables(Nft),
}

impl Backend {
    /// The real backend `ALLOWLIST_FIREWALL` names, once this process is seen to be able to use it
    /// (`server.ts`): nftables needs Linux and an nft that can list the tables, Windows Firewall
    /// needs Windows and administrator rights.
    pub async fn real(config: &AllowlistConfig) -> Result<Self, String> {
        match config.firewall {
            FirewallKind::Nftables => {
                if !cfg!(target_os = "linux") {
                    return Err("ALLOWLIST_FIREWALL=nftables needs Linux".into());
                }
                let binary = &config.nft.binary;
                if !super::nft::can_manage_nft(binary).await {
                    return Err(format!(
                        "{} could not list the firewall: install nftables (apt install nftables) and run the helper as root or with CAP_NET_ADMIN (the reforged-allowlist systemd unit does)",
                        binary.display()
                    ));
                }
                Ok(Self::Nftables(Nft::new(config.nft.clone())))
            }
            FirewallKind::Windows => {
                #[cfg(windows)]
                {
                    if !super::windows::is_elevated() {
                        return Err("ALLOWLIST_DRY_RUN=0 needs administrator rights: run it as a scheduled task with \"Run with highest privileges\"".into());
                    }
                    Ok(Self::WindowsFirewall)
                }
                #[cfg(not(windows))]
                {
                    Err("ALLOWLIST_FIREWALL=windows needs Windows (Windows Firewall); on Linux use ALLOWLIST_FIREWALL=nftables".into())
                }
            }
        }
    }

    /// A dry run that describes what the configured firewall would get.
    pub fn dry_run(config: &AllowlistConfig) -> Self {
        Self::DryRun {
            firewall: config.firewall,
            nft: config.nft.clone(),
        }
    }

    pub fn firewall(&self) -> FirewallKind {
        match self {
            Self::DryRun { firewall, .. } => *firewall,
            #[cfg(windows)]
            Self::WindowsFirewall => FirewallKind::Windows,
            Self::Nftables(_) => FirewallKind::Nftables,
        }
    }

    pub fn name(&self) -> &'static str {
        match self.firewall() {
            FirewallKind::Windows => "windows-firewall",
            FirewallKind::Nftables => "linux-nftables",
        }
    }

    /// What a dry run writes to the audit log for `rule`: for nftables the exact script, for Windows
    /// Firewall the rule's settings.
    pub fn describe(&self, rule: &RulePlan) -> Result<Value, String> {
        let mut out = json!({
            "firewall": self.firewall().as_str(),
            "rule": match self.firewall() {
                FirewallKind::Windows => RULE_DISPLAY_NAME,
                FirewallKind::Nftables => "",
            },
            "enabled": rule.enabled(),
            "ips": rule.ips,
            "ports": rule.ports,
        });
        match self {
            Self::DryRun {
                firewall: FirewallKind::Nftables,
                nft,
            } => {
                out["rule"] = json!(format!("table inet {}", nft.table));
                out["script"] = json!(nft_script(rule, nft)?);
            }
            Self::Nftables(nft) => {
                out["rule"] = json!(format!("table inet {}", nft.settings().table));
                out["script"] = json!(nft_script(rule, nft.settings())?);
            }
            _ => out["windows"] = windows_change(rule).to_json(),
        }
        Ok(out)
    }

    /// Changes the firewall (never for a dry run, which the caller logs instead).
    pub async fn apply(&self, rule: &RulePlan) -> Result<(), String> {
        match self {
            Self::DryRun { .. } => Ok(()),
            #[cfg(windows)]
            Self::WindowsFirewall => {
                let change = windows_change(rule);
                tokio::task::spawn_blocking(move || super::windows::apply_rule(RULE_DISPLAY_NAME, &change))
                    .await
                    .map_err(|e| e.to_string())?
            }
            Self::Nftables(nft) => nft.apply(rule).await,
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
    fn the_rule_is_named_as_the_node_helper_named_it() {
        assert_eq!(RULE_NAME, "ReforgedRevival-GamePorts-Allowlist");
        assert_eq!(RULE_DISPLAY_NAME, "Dauntless Reforged Revival game ports (allowlist)");
    }

    #[test]
    fn plans_only_canonical_single_addresses() {
        let rule = plan(&strings(&["203.0.113.7", "2001:db8::1"]), "8770-8777").unwrap();
        assert!(rule.enabled());
        assert!(!plan(&[], "8770-8777").unwrap().enabled());
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
                plan(&strings(&[bad]), "8770-8777").unwrap_err().contains("refusing"),
                "{bad}"
            );
        }
        assert!(
            plan(&strings(&["203.0.113.7"]), "8770-8777; Remove-NetFirewallRule")
                .unwrap_err()
                .contains("ALLOWLIST_PORTS")
        );
    }

    #[test]
    fn the_windows_rule_after_a_login_and_after_a_logout() {
        // A player logs in: one inbound UDP allow rule for the game ports, only from them
        let login = plan(&strings(&["203.0.113.7", "2001:db8::1"]), "8770-8777").unwrap();
        assert_eq!(
            windows_change(&login),
            WindowsChange::Open(WindowsRule {
                display_name: "Dauntless Reforged Revival game ports (allowlist)",
                description: RULE_DESCRIPTION,
                grouping: "Dauntless Reforged Revival",
                direction: "in",
                action: "allow",
                protocol: "udp",
                local_ports: "8770-8777".into(),
                remote_addresses: "203.0.113.7,2001:db8::1".into(),
                profiles: "all",
            })
        );
        // The last one's address expires: the rule is disabled, nothing else changes
        let logout = plan(&[], "8770-8777").unwrap();
        assert_eq!(windows_change(&logout), WindowsChange::Close);
        assert_eq!(windows_change(&logout).to_json(), json!({ "enabled": false }));
    }

    fn config(firewall: FirewallKind) -> AllowlistConfig {
        let secret = "a".repeat(40);
        let kind = firewall.as_str();
        let mut config = AllowlistConfig::from_env(&service_kit::env::Env::from_pairs([
            ("ALLOWLIST_SECRET", secret.as_str()),
            ("ALLOWLIST_DRY_RUN", "1"),
            ("ALLOWLIST_FIREWALL", kind),
        ]))
        .unwrap();
        config.nft.binary = "/usr/sbin/nft".into();
        config
    }

    #[test]
    fn a_dry_run_describes_what_the_configured_firewall_would_get() {
        let rule = plan(&strings(&["203.0.113.7"]), "8770-8777").unwrap();
        let nft = Backend::dry_run(&config(FirewallKind::Nftables));
        assert_eq!(nft.name(), "linux-nftables");
        let described = nft.describe(&rule).unwrap();
        assert_eq!(described["firewall"], "nftables");
        assert_eq!(described["rule"], "table inet reforged_revival");
        assert!(
            described["script"]
                .as_str()
                .unwrap()
                .contains("elements = { 203.0.113.7 }")
        );

        let windows = Backend::dry_run(&config(FirewallKind::Windows));
        assert_eq!(windows.name(), "windows-firewall");
        let described = windows.describe(&rule).unwrap();
        assert_eq!(described["rule"], RULE_DISPLAY_NAME);
        assert_eq!(described["windows"]["remoteAddresses"], "203.0.113.7");
        assert!(described.get("script").is_none());
    }

    #[tokio::test]
    async fn the_real_backend_wants_its_platform() {
        if !cfg!(target_os = "linux") {
            assert_eq!(
                Backend::real(&config(FirewallKind::Nftables)).await.err().as_deref(),
                Some("ALLOWLIST_FIREWALL=nftables needs Linux")
            );
        }
        if !cfg!(windows) {
            assert!(
                Backend::real(&config(FirewallKind::Windows))
                    .await
                    .err()
                    .unwrap()
                    .contains("ALLOWLIST_FIREWALL=nftables")
            );
        }
    }
}
