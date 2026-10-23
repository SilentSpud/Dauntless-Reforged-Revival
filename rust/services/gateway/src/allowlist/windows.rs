//! The Windows Firewall rule through COM (`INetFwPolicy2`), instead of the Node helper's PowerShell.
//! Only the rule named [`RULE_DISPLAY_NAME`] is ever touched: the first one is set to the plan, any
//! other with the same name is disabled, so exactly one is in effect. An empty plan disables them
//! all (Windows then drops the game ports' traffic by default). A rule the Node helper made has the
//! same display name and is taken over as it is.
//!
//! [`RULE_DISPLAY_NAME`]: super::firewall::RULE_DISPLAY_NAME

#![allow(unsafe_code)] // COM calls

use windows::Win32::Foundation::{CloseHandle, HANDLE, VARIANT_BOOL};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, INetFwRules, NET_FW_ACTION_ALLOW, NET_FW_IP_PROTOCOL_UDP, NET_FW_PROFILE2_ALL,
    NET_FW_RULE_DIR_IN, NetFwPolicy2, NetFwRule,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch,
};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};

use super::firewall::{WindowsChange, WindowsRule};

/// A rule as it is now: enabled, local ports, remote addresses, protocol, direction and action.
pub type RuleSeen = (bool, String, String, i32, i32, i32);

/// Every rule with this display name.
unsafe fn named(rules: &INetFwRules, name: &str) -> windows::core::Result<Vec<INetFwRule>> {
    let wanted = BSTR::from(name);
    let enumerator: IEnumVARIANT = unsafe { rules._NewEnum()? }.cast()?;
    let mut found = Vec::new();
    loop {
        let mut item = [VARIANT::default()];
        let mut fetched = 0u32;
        unsafe { enumerator.Next(&mut item, &mut fetched).ok()? };
        if fetched == 0 {
            return Ok(found);
        }
        let rule: INetFwRule = IDispatch::try_from(&item[0])?.cast()?;
        if unsafe { rule.Name()? } == wanted {
            found.push(rule);
        }
    }
}

/// The settings of a [`WindowsRule`]. Its direction, action, protocol and profiles are always in,
/// allow, UDP and all; the unit test of `windows_change` pins them.
unsafe fn configure(rule: &INetFwRule, wanted: &WindowsRule) -> windows::core::Result<()> {
    unsafe {
        rule.SetDescription(&BSTR::from(wanted.description))?;
        rule.SetGrouping(&BSTR::from(wanted.grouping))?;
        rule.SetDirection(NET_FW_RULE_DIR_IN)?;
        rule.SetAction(NET_FW_ACTION_ALLOW)?;
        // The protocol before the ports: ports need TCP or UDP
        rule.SetProtocol(NET_FW_IP_PROTOCOL_UDP.0)?;
        rule.SetLocalPorts(&BSTR::from(wanted.local_ports.as_str()))?;
        rule.SetRemoteAddresses(&BSTR::from(wanted.remote_addresses.as_str()))?;
        rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
        rule.SetEnabled(VARIANT_BOOL::from(true))
    }
}

fn policy() -> windows::core::Result<INetFwPolicy2> {
    unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }
}

unsafe fn apply(name: &str, change: &WindowsChange) -> windows::core::Result<()> {
    unsafe {
        let rules = policy()?.Rules()?;
        let ours = named(&rules, name)?;
        let WindowsChange::Open(wanted) = change else {
            for rule in &ours {
                rule.SetEnabled(VARIANT_BOOL::from(false))?;
            }
            return Ok(());
        };
        match ours.split_first() {
            Some((rule, others)) => {
                for other in others {
                    other.SetEnabled(VARIANT_BOOL::from(false))?;
                }
                configure(rule, wanted)
            }
            None => {
                let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;
                rule.SetName(&BSTR::from(name))?;
                configure(&rule, wanted)?;
                rules.Add(&rule)
            }
        }
    }
}

unsafe fn read(name: &str) -> windows::core::Result<Vec<RuleSeen>> {
    unsafe {
        let mut out = Vec::new();
        for rule in named(&policy()?.Rules()?, name)? {
            out.push((
                rule.Enabled()?.as_bool(),
                rule.LocalPorts()?.to_string(),
                rule.RemoteAddresses()?.to_string(),
                rule.Protocol()?,
                rule.Direction()?.0,
                rule.Action()?.0,
            ));
        }
        Ok(out)
    }
}

unsafe fn remove(name: &str) -> windows::core::Result<()> {
    unsafe {
        let rules = policy()?.Rules()?;
        // Remove takes one rule of that name at a time
        while !named(&rules, name)?.is_empty() {
            rules.Remove(&BSTR::from(name))?;
        }
        Ok(())
    }
}

fn with_com<T>(work: impl FnOnce() -> windows::core::Result<T>) -> Result<T, String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("COM: {e}"))?;
        let result = work().map_err(|e| format!("Windows Firewall: {e}"));
        CoUninitialize();
        result
    }
}

/// Brings the rules named `name` to `change` (blocking: run it off the async threads).
pub fn apply_rule(name: &str, change: &WindowsChange) -> Result<(), String> {
    with_com(|| unsafe { apply(name, change) })
}

/// Every rule named `name` as it is now. Read-only.
pub fn read_rules(name: &str) -> Result<Vec<RuleSeen>, String> {
    with_com(|| unsafe { read(name) })
}

/// Deletes every rule named `name`. Only the live test calls it, for its own rule.
pub fn remove_rules(name: &str) -> Result<(), String> {
    with_com(|| unsafe { remove(name) })
}

/// Read-only: does this process run with administrator rights? (Real mode refuses to start without
/// them, because every firewall change would fail.)
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        read.is_ok() && elevation.TokenIsElevated != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allowlist::firewall::{RULE_DISPLAY_NAME, plan, windows_change};

    #[test]
    fn the_elevation_check_answers() {
        // Read-only; either answer is fine
        let _ = is_elevated();
    }

    #[test]
    fn reading_the_rules_needs_no_rights() {
        // Read-only: the helper's own rule, if this machine has one, is only looked at
        assert!(read_rules(RULE_DISPLAY_NAME).is_ok());
    }

    /// Opt-in (`REFORGED_TEST_WINDOWS_FIREWALL=1`, an elevated prompt): really applies a login and a
    /// logout to a rule of its own on ports nothing uses, reads it back, and deletes it afterwards.
    #[test]
    fn windows_firewall_for_real() {
        if std::env::var("REFORGED_TEST_WINDOWS_FIREWALL").as_deref() != Ok("1") {
            eprintln!("set REFORGED_TEST_WINDOWS_FIREWALL=1 in an elevated prompt to run");
            return;
        }
        assert!(is_elevated(), "needs an elevated prompt");
        const NAME: &str = "Dauntless Reforged Revival allowlist test (delete me)";
        let strings = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let result = (|| {
            let login = plan(&strings(&["203.0.113.7", "2001:db8::1"]), "64770-64771")?;
            apply_rule(NAME, &windows_change(&login))?;
            let rules = read_rules(NAME)?;
            assert_eq!(rules.len(), 1, "{rules:?}");
            let (enabled, ports, remote, protocol, direction, action) = &rules[0];
            assert!(*enabled);
            assert_eq!(ports, "64770-64771");
            // Windows may write single addresses back with a mask
            assert!(
                remote.contains("203.0.113.7") && remote.contains("2001:db8::1"),
                "{remote}"
            );
            assert_eq!(
                (*protocol, *direction, *action),
                (NET_FW_IP_PROTOCOL_UDP.0, NET_FW_RULE_DIR_IN.0, NET_FW_ACTION_ALLOW.0)
            );

            // Another login updates the same rule instead of adding one
            let again = plan(&strings(&["198.51.100.9"]), "64770-64771")?;
            apply_rule(NAME, &windows_change(&again))?;
            let rules = read_rules(NAME)?;
            assert_eq!(rules.len(), 1);
            assert!(rules[0].2.contains("198.51.100.9") && !rules[0].2.contains("203.0.113.7"));

            let logout = plan(&[], "64770-64771")?;
            apply_rule(NAME, &windows_change(&logout))?;
            let rules = read_rules(NAME)?;
            assert_eq!(rules.len(), 1);
            assert!(!rules[0].0, "the logout disables the rule");
            Ok::<(), String>(())
        })();
        let removed = remove_rules(NAME);
        result.unwrap();
        removed.unwrap();
        assert!(read_rules(NAME).unwrap().is_empty(), "the test rule is gone");
    }
}
