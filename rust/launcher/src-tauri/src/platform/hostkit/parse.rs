//! What the Host page reads from text it did not write: the host kit's output, `host.toml` (or a Node
//! kit's `host.json`) and `vswhere`'s answers. Nothing here touches a file or a process, and it needs
//! only `serde_json` and `toml_edit`, so the fuzz target `launcher_host` compiles it as it is
//! (MIGRATION_PLAN.md RX3).

use serde_json::{Map, Value};

/// `MAX_MULTIPLIER` in the bridge's types; a test checks they agree.
pub const MAX_MULTIPLIER: f64 = 100.0;

/// The first `max` characters of `text` (never splits a character).
pub fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// A line of a command's output as the page shows it: terminal colours and carriage returns out, the
/// end trimmed, at most 400 characters (Reforged's `cleanLine`).
pub fn clean_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // Parameters (digits, ';', '?') up to the final letter, which ends the escape.
            while let Some(&p) = chars.peek() {
                chars.next();
                if p.is_ascii_alphabetic() {
                    break;
                }
                if !(p.is_ascii_digit() || p == ';' || p == '?') {
                    break;
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    cut(out.trim_end(), 400)
}

/// Why a command failed: the host kit's last `Error: …` line, else its last non-empty line, at most 300
/// characters (Reforged's `failureLine`).
pub fn failure_line(lines: &[String]) -> Option<String> {
    for line in lines.iter().rev() {
        if let Some(reason) = line.trim().strip_prefix("Error: ") {
            return Some(cut(reason, 300));
        }
    }
    lines
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .map(|line| cut(line, 300))
}

/// The JSON object a host kit command ends with (`--json`): the last line that starts with `{`.
pub fn last_json(lines: &[String]) -> Option<Map<String, Value>> {
    let line = lines.iter().rev().find(|line| line.trim_start().starts_with('{'))?;
    match serde_json::from_str(line.trim()) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// `invite --json`'s answer: the link (a secret) and who it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLine {
    pub link: String,
    pub for_name: Option<String>,
}

/// The invite in `invite --json`'s output. Only a `reforged-revival://` link is taken, at most 4096
/// characters, so nothing else reaches the page's copy button.
pub fn invite_line(lines: &[String]) -> Option<InviteLine> {
    let json = last_json(lines)?;
    let link = json.get("link")?.as_str()?.trim();
    if !link.starts_with("reforged-revival://") || link.len() > 4096 || link.chars().any(char::is_control) {
        return None;
    }
    let for_name = json
        .get("for")
        .and_then(Value::as_str)
        .map(|name| cut(name, 40))
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control));
    Some(InviteLine {
        link: link.to_owned(),
        for_name,
    })
}

/// `update --check --json`: how many commits (or releases) behind, and where this PC is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCheck {
    pub behind: u64,
    pub head: String,
}

pub fn update_check(json: &Map<String, Value>) -> Option<UpdateCheck> {
    let behind = json.get("behind")?.as_f64()?;
    if !behind.is_finite() || behind < 0.0 {
        return None;
    }
    Some(UpdateCheck {
        behind: behind as u64,
        head: short_text(json.get("head")),
    })
}

/// `update --json --no-setup`: what the update did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDone {
    pub to: String,
    pub files: u32,
    pub runtime_changed: bool,
}

pub fn update_done(json: &Map<String, Value>) -> Option<UpdateDone> {
    if json.get("updated") != Some(&Value::Bool(true)) {
        return None;
    }
    let files = json
        .get("files")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map_or(0, |n| n.min(f64::from(u32::MAX)) as u32);
    Some(UpdateDone {
        to: short_text(json.get("to")),
        files,
        runtime_changed: json.get("runtimeChanged") == Some(&Value::Bool(true)),
    })
}

/// A commit or version as text: a string or a number, at most 40 characters, printable only.
fn short_text(value: Option<&Value>) -> String {
    let text = match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    cut(&text, 40).chars().filter(|c| !c.is_control()).collect()
}

/// `update`'s own error codes (`not_git`, `local_changes`, …) become `update_<code>`, which the page
/// has text for; anything else is the failure line, or `update`.
pub fn update_failure(json: Option<&Map<String, Value>>, failure: Option<&str>) -> String {
    let code = json
        .and_then(|json| json.get("error"))
        .and_then(Value::as_str)
        .filter(|code| (1..=40).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    match (code, failure) {
        (Some(code), _) => format!("update_{code}"),
        (None, Some(failure)) => failure.to_owned(),
        (None, None) => "update".to_owned(),
    }
}

/// `publish-runtime`'s version (`Runtime 20261014-1a2b3c4d published in …`).
pub fn published_version(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        let rest = line.trim().strip_prefix("Runtime ")?;
        let (version, _) = rest.split_once(" published in ")?;
        let ok = (1..=40).contains(&version.len())
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
        ok.then(|| version.to_owned())
    })
}

/// The file name `gamedata --export` wrote (`Game data pack written: <path>`).
pub fn pack_written(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        let path = line.trim().strip_prefix("Game data pack written: ")?;
        let name = path.rsplit(['\\', '/']).next()?.trim();
        (!name.is_empty()).then(|| cut(name, 120))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMode {
    Private,
    Public,
}

/// What the Host page shows of `.workspace/config/host.json` (no secrets are in it).
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub name: Option<String>,
    pub mode: ServerMode,
    pub host: Option<String>,
    /// (xp, rewards), private servers only, None when off
    pub multipliers: Option<(f64, f64)>,
}

/// `host.json` as Reforged's `readServerConfig` reads it: None unless `mode` is `private` or `public`.
pub fn server_config(text: &str) -> Option<ServerConfig> {
    let Ok(Value::Object(c)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    server_config_of(&c)
}

/// `host.toml`, read as [`server_config`] reads host.json: the keys the Host page shows are named the
/// same in both files.
pub fn server_config_toml(text: &str) -> Option<ServerConfig> {
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let mut c = Map::new();
    for key in ["name", "mode", "host"] {
        if let Some(s) = doc.get(key).and_then(toml_edit::Item::as_str) {
            c.insert(key.into(), Value::String(s.to_owned()));
        }
    }
    if let Some(m) = doc.get("multipliers").and_then(toml_edit::Item::as_table_like) {
        let mut multipliers = Map::new();
        for key in ["xp", "rewards"] {
            let item = m.get(key);
            let n = item
                .and_then(toml_edit::Item::as_float)
                .or_else(|| item.and_then(toml_edit::Item::as_integer).map(|n| n as f64));
            if let Some(number) = n.and_then(serde_json::Number::from_f64) {
                multipliers.insert(key.into(), Value::Number(number));
            }
        }
        c.insert("multipliers".into(), Value::Object(multipliers));
    }
    server_config_of(&c)
}

fn server_config_of(c: &Map<String, Value>) -> Option<ServerConfig> {
    let mode = match c.get("mode").and_then(Value::as_str) {
        Some("private") => ServerMode::Private,
        Some("public") => ServerMode::Public,
        _ => return None,
    };
    let text = |key: &str, max: usize| c.get(key).and_then(Value::as_str).map(|s| cut(s, max));
    Some(ServerConfig {
        name: text("name", 80),
        mode,
        host: text("host", 255),
        multipliers: if mode == ServerMode::Private {
            multipliers(c.get("multipliers"))
        } else {
            None
        },
    })
}

/// Both finite, from 1 to `MAX_MULTIPLIER`; 1 and 1 is off.
fn multipliers(value: Option<&Value>) -> Option<(f64, f64)> {
    let m = value?.as_object()?;
    let ok = |key: &str| {
        m.get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (1.0..=MAX_MULTIPLIER).contains(n))
    };
    let (xp, rewards) = (ok("xp")?, ok("rewards")?);
    (xp != 1.0 || rewards != 1.0).then_some((xp, rewards))
}

/// The newest Visual Studio `vswhere -format json` lists: its name with the version
/// (`Visual Studio Community 2022 (17.14)`, at most 80 characters) and the major version.
pub fn vswhere_install(json: &str) -> Option<(String, Option<u32>)> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(json) else {
        return None;
    };
    let vs = list.first()?.as_object()?;
    let name = vs.get("displayName")?.as_str()?;
    let version = vs.get("installationVersion").and_then(Value::as_str).unwrap_or("");
    let short: Vec<&str> = version.split('.').take(2).collect();
    let shown = if version.is_empty() {
        name.to_owned()
    } else {
        format!("{name} ({})", short.join("."))
    };
    let major = version.split('.').next().and_then(|m| m.parse::<u32>().ok());
    Some((cut(&shown, 80), major))
}

/// The first `MSBuild.exe` line of `vswhere -find`.
pub fn vswhere_msbuild(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| line.to_ascii_lowercase().ends_with("msbuild.exe"))
        .map(str::to_owned)
}

/// `/p:PlatformToolset`: Mystic Paradox's projects select v145 (Visual Studio 2026, major 18); Visual
/// Studio 2022 builds them with v143, which its build guide asks for.
pub fn platform_toolset(major: Option<u32>) -> &'static str {
    if major.is_some_and(|m| m >= 18) { "v145" } else { "v143" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|&s| s.to_owned()).collect()
    }

    // hostserver.test.ts: "lines, errors and the page's form are cleaned and checked" (the line half)
    #[test]
    fn lines_and_errors_are_cleaned() {
        assert_eq!(clean_line("\u{1b}[31mred\u{1b}[0m\r"), "red");
        assert_eq!(clean_line("\u{1b}[32mmongod   | ready\u{1b}[0m"), "mongod   | ready");
        assert_eq!(clean_line(&"x".repeat(500)).len(), 400);
        assert_eq!(clean_line("é".repeat(500).as_str()).chars().count(), 400);
        assert_eq!(
            failure_line(&lines(&["a", "Error: no MongoDB", "b"])).as_deref(),
            Some("no MongoDB")
        );
        assert_eq!(failure_line(&lines(&["a", "last", "  "])).as_deref(), Some("last"));
        assert_eq!(failure_line(&[]), None);
    }

    #[test]
    fn the_last_json_line_is_the_answer() {
        let out = lines(&["noise", r#"{"link":"reforged-revival://join/abc","for":"Alex"}"#]);
        assert_eq!(
            invite_line(&out),
            Some(InviteLine {
                link: "reforged-revival://join/abc".into(),
                for_name: Some("Alex".into())
            })
        );
        assert_eq!(invite_line(&lines(&[r#"{"link":"https://evil.example"}"#])), None);
        assert_eq!(invite_line(&lines(&["{not json"])), None);
        assert_eq!(last_json(&lines(&["[1]"])), None);
    }

    #[test]
    fn update_answers() {
        let check = last_json(&lines(&[r#"{"behind":3,"head":"abc123"}"#])).unwrap();
        assert_eq!(
            update_check(&check),
            Some(UpdateCheck {
                behind: 3,
                head: "abc123".into()
            })
        );
        let done = last_json(&lines(&[
            r#"{"updated":true,"to":"def","files":12,"runtimeChanged":true}"#,
        ]))
        .unwrap();
        assert_eq!(
            update_done(&done),
            Some(UpdateDone {
                to: "def".into(),
                files: 12,
                runtime_changed: true
            })
        );
        let failed = last_json(&lines(&[r#"{"error":"local_changes"}"#])).unwrap();
        assert_eq!(update_failure(Some(&failed), Some("x")), "update_local_changes");
        let odd = last_json(&lines(&[r#"{"error":"Rm -rf"}"#])).unwrap();
        assert_eq!(update_failure(Some(&odd), Some("why")), "why");
        assert_eq!(update_failure(None, None), "update");
    }

    #[test]
    fn publish_and_export_lines() {
        let out = lines(&[
            "Runtime 20261014-1a2b3c4d published in C:\\rr\\.workspace\\runtime:",
            "  winmm.dll",
        ]);
        assert_eq!(published_version(&out).as_deref(), Some("20261014-1a2b3c4d"));
        let out = lines(&["Game data pack written: C:\\rr\\.workspace\\gamedata-packs\\pack.json.gz"]);
        assert_eq!(pack_written(&out).as_deref(), Some("pack.json.gz"));
    }

    #[test]
    fn host_json_as_readserverconfig() {
        let c = server_config(
            r#"{"name":"Friday","mode":"private","host":"100.64.0.1","multipliers":{"xp":5,"rewards":2}}"#,
        )
        .unwrap();
        assert_eq!(c.mode, ServerMode::Private);
        assert_eq!(c.multipliers, Some((5.0, 2.0)));
        let c = server_config(r#"{"mode":"public","multipliers":{"xp":5,"rewards":2}}"#).unwrap();
        assert_eq!(c.multipliers, None, "public servers have none");
        let c = server_config(r#"{"mode":"private","multipliers":{"xp":1,"rewards":1}}"#).unwrap();
        assert_eq!(c.multipliers, None, "1 and 1 is off");
        let c = server_config(r#"{"mode":"private","multipliers":{"xp":101,"rewards":2}}"#).unwrap();
        assert_eq!(c.multipliers, None);
        assert_eq!(server_config(r#"{"mode":"other"}"#), None);
        assert_eq!(server_config("[]"), None);
        let long = format!(r#"{{"mode":"public","name":"{}"}}"#, "n".repeat(200));
        assert_eq!(server_config(&long).unwrap().name.unwrap().len(), 80);
    }

    #[test]
    fn host_toml_as_host_json() {
        let c = server_config_toml(
            "# mine\nname = \"Friday\"\nmode = \"private\"\nhost = \"100.64.0.1\"\n\n[multipliers]\nxp = 5\nrewards = 2.5\n",
        )
        .unwrap();
        assert_eq!(c.name.as_deref(), Some("Friday"));
        assert_eq!(c.host.as_deref(), Some("100.64.0.1"));
        assert_eq!(c.multipliers, Some((5.0, 2.5)));
        let c = server_config_toml("mode = \"private\"\nmultipliers = { xp = 1, rewards = 1 }\n").unwrap();
        assert_eq!(c.multipliers, None, "1 and 1 is off");
        let c = server_config_toml("mode = \"public\"\n[multipliers]\nxp = 5\nrewards = 2\n").unwrap();
        assert_eq!(c.multipliers, None, "public servers have none");
        assert_eq!(server_config_toml("mode = \"other\"\n"), None);
        assert_eq!(server_config_toml("mode = 3\n"), None);
        assert_eq!(
            server_config_toml("{\"mode\":\"private\"}"),
            None,
            "host.json is not TOML"
        );
        let long = format!("mode = \"public\"\nname = \"{}\"\n", "n".repeat(200));
        assert_eq!(server_config_toml(&long).unwrap().name.unwrap().len(), 80);
    }

    // hostkit.test.ts: "build settings: v145 for Visual Studio 2026, v143 otherwise"
    #[test]
    fn visual_studio_and_toolset() {
        let json = r#"[{"displayName":"Visual Studio Community 2022","installationVersion":"17.14.36310.24"}]"#;
        assert_eq!(
            vswhere_install(json),
            Some(("Visual Studio Community 2022 (17.14)".into(), Some(17)))
        );
        assert_eq!(vswhere_install("[]"), None);
        assert_eq!(
            vswhere_msbuild("\r\nC:\\VS\\MSBuild\\Current\\Bin\\MSBuild.exe\r\n").as_deref(),
            Some("C:\\VS\\MSBuild\\Current\\Bin\\MSBuild.exe")
        );
        assert_eq!(platform_toolset(Some(18)), "v145");
        assert_eq!(platform_toolset(Some(17)), "v143");
        assert_eq!(platform_toolset(None), "v143");
    }

    #[test]
    fn the_limit_matches_the_bridge() {
        assert_eq!(MAX_MULTIPLIER, crate::bridge::types::MAX_MULTIPLIER);
    }
}
