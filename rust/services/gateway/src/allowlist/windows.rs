//! The Windows Firewall rule through COM (`INetFwPolicy2`), instead of the Node helper's PowerShell.
//! Only the rule named [`RULE_DISPLAY_NAME`] is ever touched: the first one is set to the plan, any
//! other with the same name is disabled, so exactly one is in effect. An empty plan disables them
//! all (Windows then drops the game ports' traffic by default). A rule the Node helper made has the
//! same name and is taken over as it is.

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

use super::firewall::{RULE_DISPLAY_NAME, RulePlan};

const GROUPING: &str = "Dauntless Revived";
const DESCRIPTION: &str =
    "Opens the game ports to players who logged in through the gateway. Managed by the allowlist helper.";

/// Every rule with our name.
unsafe fn our_rules(rules: &INetFwRules) -> windows::core::Result<Vec<INetFwRule>> {
    let ours = BSTR::from(RULE_DISPLAY_NAME);
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
        if unsafe { rule.Name()? } == ours {
            found.push(rule);
        }
    }
}

unsafe fn configure(rule: &INetFwRule, plan: &RulePlan) -> windows::core::Result<()> {
    unsafe {
        rule.SetDescription(&BSTR::from(DESCRIPTION))?;
        rule.SetGrouping(&BSTR::from(GROUPING))?;
        rule.SetDirection(NET_FW_RULE_DIR_IN)?;
        rule.SetAction(NET_FW_ACTION_ALLOW)?;
        // The protocol before the ports: ports need TCP or UDP
        rule.SetProtocol(NET_FW_IP_PROTOCOL_UDP.0)?;
        rule.SetLocalPorts(&BSTR::from(plan.ports.as_str()))?;
        rule.SetRemoteAddresses(&BSTR::from(plan.ips.join(",")))?;
        rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
        rule.SetEnabled(VARIANT_BOOL::from(true))
    }
}

unsafe fn apply(plan: &RulePlan) -> windows::core::Result<()> {
    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
        let rules = policy.Rules()?;
        let ours = our_rules(&rules)?;
        if !plan.enabled() {
            for rule in &ours {
                rule.SetEnabled(VARIANT_BOOL::from(false))?;
            }
            return Ok(());
        }
        match ours.split_first() {
            Some((rule, others)) => {
                for other in others {
                    other.SetEnabled(VARIANT_BOOL::from(false))?;
                }
                configure(rule, plan)
            }
            None => {
                let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;
                rule.SetName(&BSTR::from(RULE_DISPLAY_NAME))?;
                configure(&rule, plan)?;
                rules.Add(&rule)
            }
        }
    }
}

/// Brings the rule to `plan` (blocking: run it off the async threads).
pub fn apply_rule(plan: &RulePlan) -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("COM: {e}"))?;
        let result = apply(plan).map_err(|e| format!("Windows Firewall: {e}"));
        CoUninitialize();
        result
    }
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
    #[test]
    fn the_elevation_check_answers() {
        // Read-only; either answer is fine
        let _ = super::is_elevated();
    }
}
