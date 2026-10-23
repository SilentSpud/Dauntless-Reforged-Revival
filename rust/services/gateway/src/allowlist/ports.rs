//! The game's UDP port range (`src/allowlist/ports.ts`), on its own so `config` and `nft` can both
//! use it. The host kit writes `ALLOWLIST_PORTS`, widened for `--max-hunts`; this only checks it.

use serde_json::json;

/// "8770-8777" or "8777": ports 1-65535, a range low to high, nothing else. Returns the normalised
/// text: leading zeros gone, a single port kept single.
pub fn check_ports(text: &str) -> Result<String, String> {
    let shape = || {
        format!(
            "ALLOWLIST_PORTS must be a port or a range like 8770-8777 (got {})",
            json!(text)
        )
    };
    let digits = |s: &str| (1..=5).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    let (low, high) = match text.split_once('-') {
        Some((low, high)) if digits(low) && digits(high) => (low, Some(high)),
        None if digits(text) => (text, None),
        _ => return Err(shape()),
    };
    let low: u32 = low.parse().map_err(|_| shape())?;
    let high_value: u32 = high.map_or(Ok(low), str::parse).map_err(|_| shape())?;
    if low < 1 || high_value > 65535 || low > high_value {
        return Err("ALLOWLIST_PORTS must be within 1-65535, low to high".into());
    }
    Ok(if high.is_none() {
        low.to_string()
    } else {
        format!("{low}-{high_value}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_or_a_range_low_to_high() {
        assert_eq!(check_ports("8777").unwrap(), "8777");
        assert_eq!(check_ports("8770-8777").unwrap(), "8770-8777");
        assert_eq!(check_ports("08770-08777").unwrap(), "8770-8777");
        // What `--max-hunts` widens the default to: the host kit writes it, the helper takes it
        assert_eq!(check_ports("8770-8801").unwrap(), "8770-8801");
        assert_eq!(check_ports("1-65535").unwrap(), "1-65535");
        for bad in [
            "8777-8770",
            "0-5",
            "0",
            "8770-70000",
            "8770,8771",
            "any",
            "",
            "-",
            "8770-",
            "-8777",
            "123456",
            " 8777",
            "8770-8777 accept",
            "8770-8777; Remove-NetFirewallRule",
            "+8777",
        ] {
            assert!(check_ports(bad).unwrap_err().contains("ALLOWLIST_PORTS"), "{bad}");
        }
    }
}
