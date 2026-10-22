//! Loading Dumper-7 or CatalogExporter into the game through the runtime's own loader (RD9; replaces
//! Reforged's `src/main/injector.ts`). The loader (`winmm.dll`, `native/runtime-loader`) loads every
//! DLL listed in `mystic_loader.ini` beside itself when the game starts. For a session the launcher
//! copies the DLL the host picked next to the loader, writes the ini naming only it, starts the game,
//! and removes both when that game exits. A Dumper-7 session also sets the runtime DLLs aside, so the
//! loader loads the picked DLL alone; they go back afterwards.
//!
//! No other process is touched: there is no injection code in the launcher.
//!
//! What a session put in the game folder is written down in a record (`userData/host-loader-session.json`)
//! before anything is copied, so a launcher that was closed mid-session puts the folder back at its next
//! start ([`recover`]). Before every ordinary PLAY, [`leftovers`] lists anything still there; PLAY refuses
//! while the list is not empty, so a leftover never rides along with the runtime.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The list the loader reads beside itself.
pub const LOADER_INI: &str = "mystic_loader.ini";
/// The loader: the game loads it in place of Windows' own `winmm.dll`.
pub const LOADER_DLL: &str = "winmm.dll";
/// The runtime DLLs the loader looks for first (`native/runtime-loader`'s `RUNTIME_DLL_NAMES`).
pub const RUNTIME_DLLS: [&str; 2] = ["MysticParadox.dll", "MystPaxInternalServer.dll"];
/// CatalogExporter's file name, as the workspace builds it.
pub const EXPORTER_DLL: &str = "CatalogExporter.dll";
/// What a set-aside runtime DLL is renamed to while a Dumper-7 session runs.
const ASIDE_SUFFIX: &str = ".rr-dump-off";
/// The record's file name in the launcher's user data folder.
pub const RECORD_FILE: &str = "host-loader-session.json";
/// A record or an ini bigger than this is not the launcher's.
const MAX_TEXT: u64 = 64 * 1024;

/// What the session loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Dumper-7 into a game without the runtime
    Dump,
    /// CatalogExporter into a game with the runtime, started as PLAY starts it
    Export,
}

/// Why a session could not begin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// Something from an earlier session (or someone else) is beside the loader: these files
    Leftovers(Vec<String>),
    /// No loader in the game folder: the runtime is not installed
    NoLoader,
    /// The picked file is not a DLL the launcher may place there (no `.dll` name, or one of the
    /// loader's or runtime's own names)
    BadName,
    /// Copying, renaming or writing failed; everything done so far was undone
    Io(String),
}

impl SessionError {
    /// The `host_failed` detail the page words.
    pub fn detail(&self) -> String {
        match self {
            Self::Leftovers(files) => format!("loader_leftovers:{}", files.join(", ")),
            Self::NoLoader => "loader_missing".into(),
            Self::BadName => "loader_dll_name".into(),
            Self::Io(reason) => format!("loader_files:{reason}"),
        }
    }
}

/// What a session placed, so it can be undone, now or at the next start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderSession {
    pub kind: SessionKind,
    /// The game's Win64 folder, where the loader is
    pub dir: PathBuf,
    /// The copied DLL's file name, beside the loader
    pub dll: String,
    /// The runtime DLLs set aside: (now, where it goes back)
    pub aside: Vec<(PathBuf, PathBuf)>,
    /// Where this record is kept
    #[serde(skip)]
    pub record: PathBuf,
}

fn is_file(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|m| m.is_file())
}

fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The loader's or the runtime's own file name: never a session's DLL, never a leftover.
fn is_own(name: &str) -> bool {
    RUNTIME_DLLS.iter().chain([&LOADER_DLL]).any(|own| same_name(own, name))
}

/// The DLL names an ini lists, read with the loader's own rules: one per line, `;` and `#` start a
/// comment, `[...]` lines are skipped, and `key = value` lines give the value.
pub fn ini_entries(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.split([';', '#']).next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('[') {
                return None;
            }
            let value = line.split_once('=').map_or(line, |(_, value)| value.trim());
            (!value.is_empty()).then(|| value.to_owned())
        })
        .collect()
}

/// A plain file name the loader would find beside itself; None for paths, which never name a file the
/// launcher placed.
fn plain_name(entry: &str) -> Option<&str> {
    let name = Path::new(entry).file_name()?.to_str()?;
    (name == entry).then_some(name)
}

fn read_small(path: &Path) -> Option<String> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() && meta.len() <= MAX_TEXT => std::fs::read_to_string(path).ok(),
        _ => None,
    }
}

/// What must not be beside the loader when an ordinary PLAY starts the game: the ini, any DLL a leftover
/// ini names, any DLL an unfinished session placed, a runtime DLL a session set aside, and any DLL
/// with one of `known` names (the host's Dumper-7 and CatalogExporter). The file names, sorted.
pub fn leftovers(dir: &Path, record: &Path, known: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if is_file(&dir.join(name)) && !found.iter().any(|f| same_name(f, name)) {
            found.push(name.to_owned());
        }
    };
    add(LOADER_INI);
    if let Some(text) = read_small(&dir.join(LOADER_INI)) {
        for entry in ini_entries(&text) {
            if let Some(name) = plain_name(&entry).filter(|n| !is_own(n)) {
                add(name);
            }
        }
    }
    if let Some(session) = read_record(record).filter(|s| s.dir == dir) {
        add(&session.dll);
        for (now, _) in &session.aside {
            if let Some(name) = now.file_name().and_then(|n| n.to_str()) {
                add(name);
            }
        }
    }
    for name in known.iter().filter(|n| !is_own(n)) {
        add(name);
    }
    for name in RUNTIME_DLLS {
        add(&format!("{name}{ASIDE_SUFFIX}"));
    }
    found.sort_by_key(|name| name.to_lowercase());
    found
}

fn read_record(record: &Path) -> Option<LoaderSession> {
    let text = read_small(record)?;
    let mut session: LoaderSession = serde_json::from_str(&text).ok()?;
    session.record = record.to_path_buf();
    // A record only ever names a file beside the loader
    plain_name(&session.dll)?;
    Some(session)
}

fn write_record(session: &LoaderSession) -> io::Result<()> {
    if let Some(parent) = session.record.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(session).map_err(io::Error::other)?;
    std::fs::write(&session.record, text)
}

/// Begins a session: checks the folder is clean, writes the record, sets the runtime aside for a dump,
/// copies `picked` beside the loader and writes the ini naming only it. On any failure what was done is
/// undone.
pub fn begin(
    kind: SessionKind,
    dir: &Path,
    picked: &Path,
    record: &Path,
    known: &[&str],
) -> Result<LoaderSession, SessionError> {
    let name = picked
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.len() > 4 && n.to_ascii_lowercase().ends_with(".dll"))
        .ok_or(SessionError::BadName)?;
    if is_own(name) {
        return Err(SessionError::BadName);
    }
    let found = leftovers(dir, record, known);
    // A file of the picked name already there is a leftover too: it is never overwritten, nor later removed
    let clash = is_file(&dir.join(name)) && !found.iter().any(|f| same_name(f, name));
    if !found.is_empty() || clash {
        let mut found = found;
        if clash {
            found.push(name.to_owned());
        }
        return Err(SessionError::Leftovers(found));
    }
    if !is_file(&dir.join(LOADER_DLL)) {
        return Err(SessionError::NoLoader);
    }
    let mut session = LoaderSession {
        kind,
        dir: dir.to_path_buf(),
        dll: name.to_owned(),
        aside: Vec::new(),
        record: record.to_path_buf(),
    };
    if kind == SessionKind::Dump {
        session.aside = RUNTIME_DLLS
            .iter()
            .map(|n| dir.join(n))
            .filter(|p| is_file(p))
            .map(|p| (PathBuf::from(format!("{}{ASIDE_SUFFIX}", p.display())), p))
            .collect();
    }
    let io = |error: io::Error| SessionError::Io(crate::platform::log::describe_error(&error));
    // Written first: from here on a closed launcher still finds what to undo
    write_record(&session).map_err(io)?;
    let placed = (|| -> io::Result<()> {
        for (aside, original) in &session.aside {
            std::fs::rename(original, aside)?;
        }
        std::fs::copy(picked, dir.join(name))?;
        let text = format!(
            "; Written by the Dauntless Reforged Revival launcher for one {} session; removed when the game exits.\r\n{name}\r\n",
            match kind {
                SessionKind::Dump => "Dumper-7",
                SessionKind::Export => "CatalogExporter",
            }
        );
        std::fs::write(dir.join(LOADER_INI), text)
    })();
    if let Err(error) = placed {
        let _ = session.clone().end();
        return Err(io(error));
    }
    Ok(session)
}

impl LoaderSession {
    /// Puts the folder back: the ini and the copied DLL removed, the runtime back in place, the record
    /// gone. Every step is tried; the first error is returned (the record stays then, for the next try).
    pub fn end(self) -> io::Result<()> {
        let mut first: Option<io::Error> = None;
        let mut note = |result: io::Result<()>| {
            if let Err(error) = result
                && error.kind() != io::ErrorKind::NotFound
                && first.is_none()
            {
                first = Some(error);
            }
        };
        note(std::fs::remove_file(self.dir.join(LOADER_INI)));
        note(std::fs::remove_file(self.dir.join(&self.dll)));
        for (aside, original) in &self.aside {
            // A runtime installed since then is newer than the one set aside
            note(if is_file(original) {
                std::fs::remove_file(aside)
            } else {
                std::fs::rename(aside, original)
            });
        }
        match first {
            Some(error) => Err(error),
            None => {
                let removed = std::fs::remove_file(&self.record);
                match removed {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                }
            }
        }
    }
}

/// Undoes a session the launcher did not finish (closed while the game ran). None when there was none.
pub fn recover(record: &Path) -> Option<io::Result<()>> {
    read_record(record).map(LoaderSession::end)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Folder {
        _temp: tempfile::TempDir,
        dir: PathBuf,
        record: PathBuf,
        tool: PathBuf,
    }

    /// A Win64 folder with the loader and the runtime, and a Dumper-7 DLL elsewhere.
    fn folder() -> Folder {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Win64");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LOADER_DLL), b"loader").unwrap();
        std::fs::write(dir.join("MysticParadox.dll"), b"runtime").unwrap();
        std::fs::write(dir.join("steam_api64.dll"), b"the game's own").unwrap();
        let tools = temp.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        let tool = tools.join("Dumper-7.dll");
        std::fs::write(&tool, b"dumper").unwrap();
        Folder {
            record: temp.path().join("user").join(RECORD_FILE),
            dir,
            tool,
            _temp: temp,
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_ini_reads_as_the_loader_reads_it() {
        let text =
            "[dlls]\r\n; a comment\r\n# another\r\nfirst.dll\r\n  extra = second.dll ; why\r\n\r\nC:\\x\\third.dll\r\n";
        assert_eq!(ini_entries(text), ["first.dll", "second.dll", "C:\\x\\third.dll"]);
        assert_eq!(plain_name("first.dll"), Some("first.dll"));
        assert_eq!(plain_name("..\\first.dll"), None);
        assert_eq!(plain_name("sub/first.dll"), None);
    }

    #[test]
    fn a_dump_session_places_the_dll_and_ini_without_the_runtime_and_removes_them() {
        let f = folder();
        let session = begin(SessionKind::Dump, &f.dir, &f.tool, &f.record, &["Dumper-7.dll"]).unwrap();
        assert_eq!(
            names(&f.dir),
            [
                "Dumper-7.dll",
                "MysticParadox.dll.rr-dump-off",
                LOADER_INI,
                "steam_api64.dll",
                LOADER_DLL
            ]
        );
        let ini = std::fs::read_to_string(f.dir.join(LOADER_INI)).unwrap();
        assert_eq!(ini_entries(&ini), ["Dumper-7.dll"]);
        assert_eq!(std::fs::read(f.dir.join("Dumper-7.dll")).unwrap(), b"dumper");
        assert!(f.record.exists());
        // While it runs, PLAY would refuse
        assert_eq!(
            leftovers(&f.dir, &f.record, &[]),
            ["Dumper-7.dll", LOADER_INI, "MysticParadox.dll.rr-dump-off"]
        );

        session.end().unwrap();
        assert_eq!(names(&f.dir), ["MysticParadox.dll", "steam_api64.dll", LOADER_DLL]);
        assert_eq!(std::fs::read(f.dir.join("MysticParadox.dll")).unwrap(), b"runtime");
        assert!(!f.record.exists());
        assert!(leftovers(&f.dir, &f.record, &["Dumper-7.dll", EXPORTER_DLL]).is_empty());
    }

    #[test]
    fn an_export_session_keeps_the_runtime() {
        let f = folder();
        let exporter = f.tool.with_file_name(EXPORTER_DLL);
        std::fs::write(&exporter, b"exporter").unwrap();
        let session = begin(SessionKind::Export, &f.dir, &exporter, &f.record, &[EXPORTER_DLL]).unwrap();
        assert!(f.dir.join("MysticParadox.dll").exists());
        assert!(session.aside.is_empty());
        let ini = std::fs::read_to_string(f.dir.join(LOADER_INI)).unwrap();
        assert_eq!(ini_entries(&ini), [EXPORTER_DLL]);
        session.end().unwrap();
        assert_eq!(names(&f.dir), ["MysticParadox.dll", "steam_api64.dll", LOADER_DLL]);
    }

    #[test]
    fn a_session_the_launcher_did_not_finish_is_undone_from_its_record() {
        let f = folder();
        let session = begin(SessionKind::Dump, &f.dir, &f.tool, &f.record, &[]).unwrap();
        drop(session);
        assert!(recover(&f.record).unwrap().is_ok());
        assert_eq!(names(&f.dir), ["MysticParadox.dll", "steam_api64.dll", LOADER_DLL]);
        assert!(recover(&f.record).is_none());
    }

    #[test]
    fn a_runtime_installed_during_a_dump_session_wins_over_the_one_set_aside() {
        let f = folder();
        let session = begin(SessionKind::Dump, &f.dir, &f.tool, &f.record, &[]).unwrap();
        std::fs::write(f.dir.join("MysticParadox.dll"), b"newer").unwrap();
        session.end().unwrap();
        assert_eq!(names(&f.dir), ["MysticParadox.dll", "steam_api64.dll", LOADER_DLL]);
        assert_eq!(std::fs::read(f.dir.join("MysticParadox.dll")).unwrap(), b"newer");
    }

    #[test]
    fn leftovers_are_the_ini_what_it_names_and_the_hosts_tools() {
        let f = folder();
        assert!(leftovers(&f.dir, &f.record, &["Dumper-7.dll", EXPORTER_DLL]).is_empty());

        std::fs::write(
            f.dir.join(LOADER_INI),
            "evil.dll\r\n..\\outside.dll\r\nMysticParadox.dll\r\n",
        )
        .unwrap();
        std::fs::write(f.dir.join("evil.dll"), b"x").unwrap();
        assert_eq!(leftovers(&f.dir, &f.record, &[]), ["evil.dll", LOADER_INI]);
        std::fs::remove_file(f.dir.join(LOADER_INI)).unwrap();
        // Without an ini the loader never loads evil.dll, but a DLL the host's tools are named after is
        // still refused: it is an unknown DLL beside the loader
        assert!(leftovers(&f.dir, &f.record, &[]).is_empty());
        std::fs::write(f.dir.join(EXPORTER_DLL), b"x").unwrap();
        assert_eq!(leftovers(&f.dir, &f.record, &[EXPORTER_DLL]), [EXPORTER_DLL]);
        // The loader's and runtime's own names are never "known" leftovers
        std::fs::remove_file(f.dir.join(EXPORTER_DLL)).unwrap();
        assert!(leftovers(&f.dir, &f.record, &[LOADER_DLL, "MysticParadox.dll"]).is_empty());
    }

    #[test]
    fn a_session_refuses_leftovers_clashes_and_own_names_and_needs_the_loader() {
        let f = folder();
        std::fs::write(f.dir.join(LOADER_INI), "").unwrap();
        assert_eq!(
            begin(SessionKind::Dump, &f.dir, &f.tool, &f.record, &[]),
            Err(SessionError::Leftovers(vec![LOADER_INI.into()]))
        );
        std::fs::remove_file(f.dir.join(LOADER_INI)).unwrap();

        // A file of the picked name is never overwritten
        std::fs::write(f.dir.join("Dumper-7.dll"), b"someone else's").unwrap();
        assert_eq!(
            begin(SessionKind::Dump, &f.dir, &f.tool, &f.record, &[]),
            Err(SessionError::Leftovers(vec!["Dumper-7.dll".into()]))
        );
        assert_eq!(std::fs::read(f.dir.join("Dumper-7.dll")).unwrap(), b"someone else's");
        std::fs::remove_file(f.dir.join("Dumper-7.dll")).unwrap();

        for bad in ["winmm.dll", "MYSTICPARADOX.DLL", "notes.txt", ".dll"] {
            let picked = f.tool.with_file_name(bad);
            assert_eq!(
                begin(SessionKind::Dump, &f.dir, &picked, &f.record, &[]),
                Err(SessionError::BadName),
                "{bad}"
            );
        }

        std::fs::remove_file(f.dir.join(LOADER_DLL)).unwrap();
        assert_eq!(
            begin(SessionKind::Dump, &f.dir, &f.tool, &f.record, &[]),
            Err(SessionError::NoLoader)
        );
        assert!(!f.record.exists());
    }

    #[test]
    fn a_failed_copy_undoes_the_session() {
        let f = folder();
        let missing = f.tool.with_file_name("Gone.dll");
        let Err(SessionError::Io(_)) = begin(SessionKind::Dump, &f.dir, &missing, &f.record, &[]) else {
            panic!("the copy fails");
        };
        assert_eq!(names(&f.dir), ["MysticParadox.dll", "steam_api64.dll", LOADER_DLL]);
        assert!(!f.record.exists());
    }

    #[test]
    fn a_record_naming_a_path_is_ignored() {
        let f = folder();
        std::fs::create_dir_all(f.record.parent().unwrap()).unwrap();
        let text = serde_json::json!({
            "kind": "dump", "dir": f.dir, "dll": "..\\..\\important.dll", "aside": []
        });
        std::fs::write(&f.record, text.to_string()).unwrap();
        assert!(recover(&f.record).is_none());
    }

    #[test]
    fn the_details_are_the_pages() {
        assert_eq!(
            SessionError::Leftovers(vec![LOADER_INI.into(), "x.dll".into()]).detail(),
            "loader_leftovers:mystic_loader.ini, x.dll"
        );
        assert_eq!(SessionError::NoLoader.detail(), "loader_missing");
        assert_eq!(SessionError::BadName.detail(), "loader_dll_name");
    }
}
