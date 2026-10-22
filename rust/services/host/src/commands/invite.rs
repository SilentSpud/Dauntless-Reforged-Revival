//! `reforged-host invite`: the Node kit's `invite()` in `cli.ts`, with [`crate::invite`].

use std::process::ExitCode;

use serde_json::{Value, json};

use crate::Outcome;
use crate::cli::InviteArgs;
use crate::commands::setup::options::{number, number_value, text};
use crate::error::HostError;
use crate::hostconfig::{HostConfig, ServerMode, load_or_create_secrets, read_host_config, stringify_pretty};
use crate::invite::{CodeOptions, certificate_fingerprint, create_invite_code, invite_for, list_invite_codes};
use crate::paths::{PORTS, Paths};

pub fn run(paths: &Paths, args: &InviteArgs) -> Outcome {
    run_with(paths, args, PORTS.backend, &mut |line| println!("{line}"))
}

/// `uses` as `--uses` gives it: a whole number from 1 to 1000.
pub fn uses(args: &InviteArgs) -> Result<f64, HostError> {
    let raw = text(&Some(args.uses.clone())).unwrap_or("1").to_owned();
    let uses = number(&raw);
    if uses.fract() != 0.0 || !(1.0..=1000.0).contains(&uses) {
        return Err(HostError::message("--uses must be a whole number from 1 to 1000"));
    }
    Ok(uses)
}

/// The text the Node kit prints for an invite (`--json`: one line for the launcher's Host page).
pub fn invite_text(config: &HostConfig, args: &InviteArgs, uses: f64, link: &str) -> Vec<String> {
    let whom = text(&args.for_whom);
    if args.json {
        let line = json!({
            "link": link,
            "for": whom,
            "uses": if args.unlimited { Value::Null } else { number_value(uses) },
            "shareUrl": if config.mode == ServerMode::Private { json!(config.share_url) } else { Value::Null },
        });
        return vec![service_kit::jsjson::stringify(&line)];
    }
    let mut out = vec![format!(
        "\nInvite{} ({}):\n",
        whom.map(|w| format!(" for {w}")).unwrap_or_default(),
        if args.unlimited {
            "unlimited uses".to_owned()
        } else {
            format!(
                "{} use{}",
                super::setup::options::number_text(uses),
                if uses == 1.0 { "" } else { "s" }
            )
        }
    )];
    out.push(format!("  {link}\n"));
    out.push(
        "Send the whole line. They paste it into the Dauntless Reforged Revival launcher (or click it once the launcher is installed)."
            .into(),
    );
    if config.mode == ServerMode::Private {
        out.push(format!(
            "They also need Tailscale, and access to this PC: share it from https://login.tailscale.com/admin/machines{}.",
            if config.share_url.is_some() {
                ""
            } else {
                " (or put the share link in invites with reforged-host setup --share <link>)"
            }
        ));
    }
    out
}

/// The command with the backend's port given (tests run a fake backend).
pub fn run_with(paths: &Paths, args: &InviteArgs, backend_port: u16, out: &mut dyn FnMut(&str)) -> Outcome {
    let config = read_host_config(&paths.host_json)?
        .ok_or_else(|| HostError::message("not set up yet: run reforged-host setup first"))?;
    let (secrets, _) = load_or_create_secrets(&paths.config, super::setup::secrets::new_secrets)?;
    if args.list {
        out(&stringify_pretty(&list_invite_codes(&secrets.admin_key, backend_port)?));
        return Ok(ExitCode::SUCCESS);
    }
    let uses = uses(args)?;
    let code = create_invite_code(
        &secrets.admin_key,
        &CodeOptions {
            uses,
            infinite: args.unlimited,
            note: text(&args.for_whom).map(str::to_owned),
        },
        backend_port,
    )?;
    let fp = match config.mode {
        ServerMode::Public => Some(certificate_fingerprint(&paths.cert_file)?),
        ServerMode::Private => None,
    };
    let link = invite_for(&config, &code, fp.as_deref())?;
    for line in invite_text(&config, args, uses, &link) {
        out(&line);
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_is_a_whole_number_from_1_to_1000() {
        let with = |u: &str| InviteArgs {
            uses: u.into(),
            ..Default::default()
        };
        assert_eq!(uses(&with("1")).unwrap(), 1.0);
        assert_eq!(uses(&with(" 25 ")).unwrap(), 25.0);
        assert_eq!(uses(&with("")).unwrap(), 1.0, "empty is the default");
        for bad in ["0", "1001", "2.5", "many"] {
            assert_eq!(
                uses(&with(bad)).unwrap_err().to_string(),
                "--uses must be a whole number from 1 to 1000",
                "{bad}"
            );
        }
    }
}
