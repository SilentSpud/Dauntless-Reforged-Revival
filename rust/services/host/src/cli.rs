//! The command line: the Node kit's commands with their options (`legacy/ReforgedHost/src/cli.ts`),
//! so an existing host's habits, scripts and the launcher's Host page keep working (section 6).
//!
//! Every option is kept as the Node kit has it. Values stay text here: each command checks its own
//! values with the Node kit's messages (`--max-hunts`, `--public-port`, `--xp x5` ...). Options
//! whose value the Node kit lets be left out (`gamedata --export [file]`, `--import`, `--from`)
//! take an optional value, so the command can answer as the Node kit does.
//!
//! RH1 owns this file; a row that needs another option goes through the integration pass.

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "reforged-host",
    about = "Dauntless Reforged Revival host kit: sets up the server, runs it and makes invites",
    after_help = "See README.md and docs/hosting.md.",
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

// Parsed once per run: boxing setup's options would only make the rows' code noisier.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Set up (or update) this server: Mystic Paradox, the services, host.json and the env files
    Setup(SetupArgs),
    /// Run every service until Ctrl+C (or a "stop" line with REFORGED_CONTROL=stdin)
    Start,
    /// Make an invite link for a player
    Invite(InviteArgs),
    /// Check what is still missing or wrong
    Doctor(DoctorArgs),
    /// Generate the game data from your export, or use the examples, or move it as a pack
    Gamedata(GamedataArgs),
    /// Publish the game runtime (MysticParadox.dll and winmm.dll) for the players
    PublishRuntime(PublishRuntimeArgs),
    /// Show, create or apply the project's server preset (preset/)
    Preset(PresetArgs),
    /// XP and reward multipliers (private test servers only)
    Multipliers(MultipliersArgs),
    /// Download and patch Mystic Paradox only (what importing the SDK needs)
    Upstream,
    /// Check for an update, apply it and run setup again
    Update(UpdateArgs),
    /// Back up the server's database
    Backup,
    /// Linux: one game server through Wine or Proton, as the Director starts it (the Node kit's
    /// `dist/gameserver.js`, run by `.workspace/bin/reforged-gameserver`). Not for people.
    #[command(hide = true)]
    Gameserver {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

impl Command {
    /// The command's name as typed.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Setup(_) => "setup",
            Self::Start => "start",
            Self::Invite(_) => "invite",
            Self::Doctor(_) => "doctor",
            Self::Gamedata(_) => "gamedata",
            Self::PublishRuntime(_) => "publish-runtime",
            Self::Preset(_) => "preset",
            Self::Multipliers(_) => "multipliers",
            Self::Upstream => "upstream",
            Self::Update(_) => "update",
            Self::Backup => "backup",
            Self::Gameserver { .. } => "gameserver",
        }
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct SetupArgs {
    /// private (friends over Tailscale) or public (a server on the internet)
    #[arg(long, value_name = "private|public")]
    pub mode: Option<String>,
    /// Private: this PC's Tailscale IP (found when Tailscale runs). Public: the public IPv4 or DNS name
    #[arg(long, value_name = "address")]
    pub host: Option<String>,
    /// The server's name
    #[arg(long, value_name = "server name")]
    pub name: Option<String>,
    /// The Dauntless 1.14.7 folder (the one with Archon), as a full path
    #[arg(long, value_name = "Dauntless 1.14.7 folder")]
    pub game: Option<String>,
    /// The IPv4 the game servers announce (a public server on a DNS name needs it)
    #[arg(long, value_name = "IPv4")]
    pub game_address: Option<String>,
    /// Who may register
    #[arg(long, value_name = "INVITECODE|OPEN|NONE")]
    pub registration: Option<String>,
    /// A Tailscale share link to put in invites (private servers)
    #[arg(long, value_name = "Tailscale share link")]
    pub share: Option<String>,
    /// The backend's MongoDB connection string
    #[arg(long, value_name = "uri")]
    pub mongo_uri: Option<String>,
    /// The host kit runs its own MongoDB (the default with the default URI)
    #[arg(long)]
    pub manage_mongo: bool,
    /// Use a MongoDB that runs on its own (with --mongo-uri)
    #[arg(long)]
    pub no_manage_mongo: bool,
    /// The mongod program, when it is not found
    #[arg(long, value_name = "path")]
    pub mongod: Option<String>,
    /// Hunts at once
    #[arg(long, value_name = "1-10")]
    pub max_hunts: Option<String>,
    /// Do not install and build the services
    #[arg(long)]
    pub skip_install: bool,
    /// Download and patch Mystic Paradox again even when it is in place
    #[arg(long)]
    pub force_upstream: bool,
    /// The gateway's port when 443 is taken (a hosting panel, for example); 443 goes back
    #[arg(long, value_name = "port", help_heading = "Public servers")]
    pub public_port: Option<String>,
    /// The wine program (a path, or a name on PATH)
    #[arg(long, value_name = "path", help_heading = "Linux")]
    pub wine: Option<String>,
    /// A Proton (or GE-Proton) script to run the game servers with instead of Wine
    #[arg(long, value_name = "script", help_heading = "Linux")]
    pub proton: Option<String>,
    /// Back to Wine
    #[arg(long, help_heading = "Linux")]
    pub no_proton: bool,
    /// A virtual display (xvfb-run) for the game servers (the default)
    #[arg(long, help_heading = "Linux")]
    pub xvfb: bool,
    /// No virtual display
    #[arg(long, help_heading = "Linux")]
    pub no_xvfb: bool,
    /// Public: the allowlist filters the game ports on this interface only
    #[arg(long, value_name = "eth0", help_heading = "Linux")]
    pub firewall_interface: Option<String>,
    /// The allowlist filters on every interface (the default)
    #[arg(long, help_heading = "Linux")]
    pub no_firewall_interface: bool,
    /// Each game server's frame rate with players on it; 0: no cap
    #[arg(long, value_name = "0|10-240", help_heading = "CPU and RAM")]
    pub server_fps: Option<String>,
    /// The frame rate after 15 s without players; 0: the same as --server-fps
    #[arg(long, value_name = "0|1-240", help_heading = "CPU and RAM")]
    pub idle_fps: Option<String>,
    /// The Training Grounds start when someone goes there (the default)
    #[arg(long, help_heading = "CPU and RAM")]
    pub training_on_demand: bool,
    /// The Training Grounds start with the server
    #[arg(long, help_heading = "CPU and RAM")]
    pub training_at_boot: bool,
    /// Warnings and errors only in the game servers' logs (the default)
    #[arg(long, help_heading = "CPU and RAM")]
    pub quiet_game_logs: bool,
    /// Upstream's diagnostic log levels in the game servers' logs
    #[arg(long, help_heading = "CPU and RAM")]
    pub verbose_game_logs: bool,
    /// A new hunt only when there is memory for it (the default)
    #[arg(long, help_heading = "CPU and RAM")]
    pub memory_guard: bool,
    /// Start hunts whatever the free memory
    #[arg(long, help_heading = "CPU and RAM")]
    pub no_memory_guard: bool,
    /// Memory left free after a new hunt
    #[arg(long, value_name = "MB", help_heading = "CPU and RAM")]
    pub min_free_ram: Option<String>,
    /// Memory one hunt needs
    #[arg(long, value_name = "MB", help_heading = "CPU and RAM")]
    pub hunt_ram: Option<String>,
    /// On demand: stop the Training Grounds after this long without players; 0: keep them running
    #[arg(long, value_name = "0-1440", help_heading = "CPU and RAM")]
    pub training_idle_minutes: Option<String>,
    /// Multiply the XP the game servers grant
    #[arg(long, value_name = "1-100", help_heading = "Test multipliers (private servers)")]
    pub xp_multiplier: Option<String>,
    /// Multiply the loot the game servers grant
    #[arg(long, value_name = "1-100", help_heading = "Test multipliers (private servers)")]
    pub reward_multiplier: Option<String>,
    /// Turns the multipliers off
    #[arg(long, help_heading = "Test multipliers (private servers)")]
    pub off: bool,
    /// Same as --off
    #[arg(long, help_heading = "Test multipliers (private servers)")]
    pub no_multipliers: bool,
    /// Words that are not options: a value with spaces that was not quoted. Setup refuses them and
    /// says to quote the value, as the Node kit does.
    #[arg(hide = true)]
    pub unexpected: Vec<String>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct InviteArgs {
    /// How many players can register with it
    #[arg(long, value_name = "N", default_value = "1")]
    pub uses: String,
    /// Any number of players
    #[arg(long)]
    pub unlimited: bool,
    /// Who the invite is for (a note on the code)
    #[arg(long = "for", value_name = "who")]
    pub for_whom: Option<String>,
    /// List the invite codes instead
    #[arg(long)]
    pub list: bool,
    /// One JSON line, for the launcher's Host page
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct DoctorArgs {
    /// Do not hash the game executable
    #[arg(long)]
    pub no_hash: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct GamedataArgs {
    /// Copy the example data where data is missing (a start-up test only)
    #[arg(long)]
    pub examples: bool,
    /// Use the example sources for missing inputs
    #[arg(long)]
    pub example_sources: bool,
    /// Only the progression config, from the progression export
    #[arg(long)]
    pub progression: bool,
    /// Write a game data pack (to the file, or the default name)
    #[arg(long, value_name = "file", num_args = 0..=1)]
    pub export: Option<Option<String>>,
    /// Import a game data pack
    #[arg(long, value_name = "file", num_args = 0..=1)]
    pub import: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct PublishRuntimeArgs {
    /// The folder with MysticParadox.dll and winmm.dll, instead of this checkout's builds
    #[arg(long, value_name = "folder", num_args = 0..=1)]
    pub from: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct PresetArgs {
    /// Make the preset from this server's game data
    #[arg(long)]
    pub create: bool,
    /// With --create: generate the game data first
    #[arg(long)]
    pub generate: bool,
    /// With --create: the preset's name
    #[arg(long, value_name = "name")]
    pub name: Option<String>,
    /// Apply the preset now
    #[arg(long)]
    pub apply: bool,
    /// With --apply: replace your own game data too
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct MultipliersArgs {
    /// Multiply the XP the game servers grant
    #[arg(long, value_name = "1-100")]
    pub xp: Option<String>,
    /// Multiply the loot the game servers grant
    #[arg(long, value_name = "1-100")]
    pub rewards: Option<String>,
    /// Same as --xp (setup's name)
    #[arg(long, value_name = "1-100")]
    pub xp_multiplier: Option<String>,
    /// Same as --rewards (setup's name)
    #[arg(long, value_name = "1-100")]
    pub reward_multiplier: Option<String>,
    /// Turns the multipliers off
    #[arg(long)]
    pub off: bool,
    /// Same as --off with --xp or --rewards (alone it changes nothing, as in the Node kit)
    #[arg(long)]
    pub no_multipliers: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct UpdateArgs {
    /// Only compare, change nothing
    #[arg(long)]
    pub check: bool,
    /// End with one JSON line (the launcher)
    #[arg(long)]
    pub json: bool,
    /// Do not run setup again after updating
    #[arg(long)]
    pub no_setup: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Command {
        let mut argv = vec!["reforged-host"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn the_tree_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn has_the_node_kits_commands() {
        let names: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        for name in [
            "setup",
            "start",
            "invite",
            "doctor",
            "gamedata",
            "publish-runtime",
            "preset",
            "multipliers",
            "upstream",
            "update",
            "backup",
        ] {
            assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
        }
    }

    #[test]
    fn takes_the_node_kits_options() {
        let Command::Setup(s) = parse(&[
            "setup",
            "--mode",
            "public",
            "--host=203.0.113.7",
            "--name",
            "Friday Hunts",
            "--public-port",
            "4443",
            "--no-xvfb",
            "--server-fps",
            "45",
            "--xp-multiplier",
            "x5",
        ]) else {
            panic!("setup")
        };
        assert_eq!(s.mode.as_deref(), Some("public"));
        assert_eq!(s.host.as_deref(), Some("203.0.113.7"));
        assert_eq!(s.public_port.as_deref(), Some("4443"));
        assert!(s.no_xvfb && !s.xvfb);
        assert_eq!(s.server_fps.as_deref(), Some("45"));
        assert_eq!(s.xp_multiplier.as_deref(), Some("x5"));
        assert!(s.unexpected.is_empty());

        let Command::Setup(s) = parse(&["setup", "--game", "D:\\Games\\My", "Dauntless"]) else {
            panic!("setup")
        };
        assert_eq!(s.unexpected, ["Dauntless"], "left to setup, which names it");

        let Command::Invite(i) = parse(&["invite", "--for", "Alex"]) else {
            panic!("invite")
        };
        assert_eq!(i.uses, "1", "--uses defaults to 1");
        assert_eq!(i.for_whom.as_deref(), Some("Alex"));

        let Command::Gamedata(g) = parse(&["gamedata", "--export"]) else {
            panic!("gamedata")
        };
        assert_eq!(g.export, Some(None), "--export without a file: the default name");
        let Command::Gamedata(g) = parse(&["gamedata", "--export", "pack.zip"]) else {
            panic!("gamedata")
        };
        assert_eq!(g.export, Some(Some("pack.zip".into())));
        let Command::PublishRuntime(p) = parse(&["publish-runtime", "--from"]) else {
            panic!("publish-runtime")
        };
        assert_eq!(
            p.from,
            Some(None),
            "setup answers --from without a folder as the Node kit does"
        );

        let Command::Multipliers(m) = parse(&["multipliers", "--xp", "5", "--rewards", "3"]) else {
            panic!("multipliers")
        };
        assert_eq!((m.xp.as_deref(), m.rewards.as_deref()), (Some("5"), Some("3")));
        let Command::Update(u) = parse(&["update", "--check", "--json"]) else {
            panic!("update")
        };
        assert!(u.check && u.json && !u.no_setup);
        let Command::Doctor(d) = parse(&["doctor", "--no-hash"]) else {
            panic!("doctor")
        };
        assert!(d.no_hash);
    }

    #[test]
    fn refuses_what_the_node_kit_never_had() {
        assert!(Cli::try_parse_from(["reforged-host", "frobnicate"]).is_err());
        assert!(Cli::try_parse_from(["reforged-host", "start", "--mode", "public"]).is_err());
    }
}
