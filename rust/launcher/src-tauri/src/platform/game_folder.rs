//! "I already have the game files" (row L30): `src/main/game-folder.ts`. Finds an existing 1.4.4 game from
//! a folder the player pasted or picked, trying only these layouts (nothing is searched): the folder
//! itself, `<folder>\Dauntless`, the folder above `Archon`, and three folders above `Archon\Binaries\Win64`.
//!
//! Only drive-letter paths are accepted on Windows, before any file-system call: a UNC, device or
//! root-relative path would make Windows contact another machine, possibly with the player's network
//! credentials. The root is kept as typed or picked (no realpath), at most `MAX_GAME_ROOT` characters.

use crate::core::pins::EXE_RELATIVE_PATH;
use crate::core::text::js_length;
use crate::platform::settings::Platform;

pub const MAX_GAME_INPUT: usize = 260;
pub const MAX_GAME_ROOT: usize = 150;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameFolder {
    Found(String),
    InvalidPath,
    NotFound,
    TooLong,
}

fn separator(platform: Platform) -> char {
    match platform {
        Platform::Windows => '\\',
        Platform::Linux => '/',
    }
}

/// `path.win32.resolve` / `path.posix.resolve` of an absolute path: separators made one, `.` and `..`
/// folded, no trailing separator except the root's.
fn normalize(path: &str, platform: Platform) -> String {
    let (root, rest) = match platform {
        Platform::Windows => (format!("{}\\", &path[..2]), &path[2..]),
        Platform::Linux => ("/".to_owned(), path),
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split(|c| c == '/' || platform == Platform::Windows && c == '\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    format!("{root}{}", parts.join(&separator(platform).to_string()))
}

/// The folder as a clean absolute path, or None. Explorer's "Copy as path" adds quotes.
pub fn clean_game_folder_input(input: &str, platform: Platform) -> Option<String> {
    let mut text = crate::core::text::js_trim(input);
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        text = crate::core::text::js_trim(&text[1..text.len() - 1]);
    }
    if text.is_empty() || js_length(text) > MAX_GAME_INPUT || text.contains('\0') {
        return None;
    }
    let bytes = text.as_bytes();
    match platform {
        Platform::Windows
            if bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/') =>
        {
            Some(normalize(text, platform))
        }
        Platform::Linux if text.starts_with('/') => Some(normalize(text, platform)),
        _ => None,
    }
}

pub fn game_root_candidates(folder: &str, platform: Platform) -> Vec<String> {
    let sep = separator(platform);
    let mut out = vec![
        folder.to_owned(),
        normalize(&format!("{folder}{sep}Dauntless"), platform),
    ];
    let name = folder.rsplit(sep).next().unwrap_or_default().to_lowercase();
    if name == "archon" {
        out.push(normalize(&format!("{folder}{sep}.."), platform));
    }
    if name == "win64" {
        out.push(normalize(&format!("{folder}{sep}..{sep}..{sep}.."), platform));
    }
    out
}

pub fn locate_existing_game(input: &str, platform: Platform) -> GameFolder {
    let Some(folder) = clean_game_folder_input(input, platform) else {
        return GameFolder::InvalidPath;
    };
    let mut too_long = false;
    for root in game_root_candidates(&folder, platform) {
        let exe = EXE_RELATIVE_PATH
            .split('/')
            .fold(std::path::PathBuf::from(&root), |path, part| path.join(part));
        if !std::fs::metadata(exe).is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        if js_length(&root) <= MAX_GAME_ROOT {
            return GameFolder::Found(root);
        }
        too_long = true;
    }
    if too_long {
        GameFolder::TooLong
    } else {
        GameFolder::NotFound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_game_in_the_known_layouts_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Games").join("Dauntless");
        let exe = EXE_RELATIVE_PATH
            .split('/')
            .fold(root.clone(), |path, part| path.join(part));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        let platform = Platform::current();
        let root_text = root.to_string_lossy().into_owned();
        let win64 = exe.parent().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            locate_existing_game(&format!("\"{root_text}\""), platform),
            GameFolder::Found(root_text.clone())
        );
        assert_eq!(
            locate_existing_game(&win64, platform),
            GameFolder::Found(root_text.clone())
        );
        let above = dir.path().join("Games").to_string_lossy().into_owned();
        assert_eq!(locate_existing_game(&above, platform), GameFolder::Found(root_text));
        assert_eq!(
            locate_existing_game(&dir.path().to_string_lossy(), platform),
            GameFolder::NotFound
        );
        assert_eq!(
            locate_existing_game("relative\\path", platform),
            GameFolder::InvalidPath
        );
    }
}
