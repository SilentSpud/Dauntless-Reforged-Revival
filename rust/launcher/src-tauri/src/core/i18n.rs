//! The backend's own text (rows L14, RA3): the dialogs the backend shows, in the player's language, from
//! the same language files the page uses (`src/shared/i18n/` in legacy/ReforgedLauncher, exported to
//! `gen/strings.json` by its `test/strings-export.test.ts`, made into `Language` and its tables by
//! build.rs). Nothing here lists the languages: a language added to the page appears by rebuilding.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the close confirmation (L5) and the file dialogs (L36) show this text"
    )
)]

include!(concat!(env!("OUT_DIR"), "/languages.rs"));

/// The keys the backend shows. Each exists in every language (a test checks it).
pub const APP_TITLE: &str = "app_title";
pub const CLOSE_TITLE: &str = "close_title";
pub const CLOSE_TEXT: &str = "close_text";
pub const CLOSE_KEEP: &str = "close_keep";
pub const CLOSE_ANYWAY: &str = "close_anyway";
/// The folder dialogs' titles, by purpose, as `src/main.ts` names them.
pub const DIALOG_GAME_FOLDER: &str = "dialog_game_folder";
pub const DIALOG_RUNTIME_FOLDER: &str = "dialog_runtime_folder";
pub const DIALOG_WORKSPACE_FOLDER: &str = "dialog_workspace_folder";
pub const DIALOG_SDK_FOLDER: &str = "dialog_sdk_folder";
pub const DIALOG_DUMPER_DLL: &str = "dialog_dumper_dll";
pub const DIALOG_GAMEDATA_PACK: &str = "dialog_gamedata_pack";

#[cfg(test)]
const USED: &[&str] = &[
    APP_TITLE,
    CLOSE_TITLE,
    CLOSE_TEXT,
    CLOSE_KEEP,
    CLOSE_ANYWAY,
    DIALOG_GAME_FOLDER,
    DIALOG_RUNTIME_FOLDER,
    DIALOG_WORKSPACE_FOLDER,
    DIALOG_SDK_FOLDER,
    DIALOG_DUMPER_DLL,
    DIALOG_GAMEDATA_PACK,
];

impl Language {
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| language.code() == code)
    }

    fn lookup(self, key: &str) -> Option<&'static str> {
        let table = self.table();
        table
            .binary_search_by(|(entry, _)| (*entry).cmp(key))
            .ok()
            .map(|index| table[index].1)
    }
}

/// `translate` in `src/shared/i18n/index.ts`: the language's text, else English, else the key itself,
/// with each `{name}` replaced.
pub fn translate(language: Language, key: &str, vars: &[(&str, &str)]) -> String {
    let mut text = language
        .lookup(key)
        .or_else(|| Language::En.lookup(key))
        .unwrap_or(key)
        .to_owned();
    for (name, value) in vars {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// `systemLanguage`: the language a first start picks from the operating system's locale ("pt-BR",
/// "pt_PT", "en-US"...): the language with that exact code, else the first one for the same language (so
/// any Portuguese locale gives `pt-BR`), else English.
pub fn system_language(locale: &str) -> Language {
    let code = locale.to_ascii_lowercase().replace('_', "-");
    let language = code.split('-').next().unwrap_or_default();
    let base = |language: Language| {
        language
            .code()
            .split('-')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    Language::ALL
        .into_iter()
        .find(|candidate| candidate.code().eq_ignore_ascii_case(&code))
        .or_else(|| Language::ALL.into_iter().find(|candidate| base(*candidate) == language))
        .unwrap_or(Language::En)
}

/// The operating system's language, as `system_language` reads it.
pub fn os_language() -> Language {
    sys_locale::get_locale().map_or(Language::En, |locale| system_language(&locale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_the_backend_shows_exists_in_every_language() {
        for language in Language::ALL {
            for key in USED {
                assert!(
                    language.lookup(key).is_some_and(|text| !text.is_empty()),
                    "{} {key}",
                    language.code()
                );
            }
        }
    }

    #[test]
    fn translates_like_the_page() {
        assert_eq!(translate(Language::PtBr, "window_close", &[]), "Fechar");
        assert_eq!(
            translate(Language::PtBr, "ready_title", &[("user", "Ana")]),
            "Bem-vindo de volta, Ana"
        );
        assert_eq!(translate(Language::En, "no_such_key", &[]), "no_such_key");
        assert_eq!(translate(Language::En, "x {a} {a}", &[("a", "1")]), "x 1 1");
        assert_eq!(Language::from_code("pt-BR"), Some(Language::PtBr));
        assert_eq!(Language::from_code("pt"), None);
        assert_eq!(Language::ALL[0], Language::En);
    }

    #[test]
    fn system_language_is_the_pages() {
        for (locale, language) in [
            ("pt-BR", Language::PtBr),
            ("pt-PT", Language::PtBr),
            ("pt_BR", Language::PtBr),
            ("PT-br", Language::PtBr),
            ("pt", Language::PtBr),
            ("ptx", Language::En),
            ("en-US", Language::En),
            ("en", Language::En),
            ("fi", Language::En),
            ("", Language::En),
        ] {
            assert_eq!(system_language(locale), language, "{locale}");
        }
    }

    #[test]
    fn every_language_has_the_english_keys_and_placeholders() {
        fn placeholders(text: &str) -> Vec<&str> {
            let mut names: Vec<&str> = text
                .split('{')
                .skip(1)
                .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
                .filter(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
                .collect();
            names.sort_unstable();
            names
        }
        let english = Language::En.table();
        for language in Language::ALL {
            let table = language.table();
            let keys =
                |table: &'static [(&'static str, &'static str)]| table.iter().map(|(key, _)| *key).collect::<Vec<_>>();
            assert_eq!(keys(table), keys(english), "{}", language.code());
            for ((key, text), (_, source)) in table.iter().zip(english) {
                assert!(!text.trim().is_empty(), "{}.{key} is empty", language.code());
                assert_eq!(placeholders(text), placeholders(source), "{}.{key}", language.code());
            }
        }
    }

    #[test]
    fn the_1_4_4_text_is_not_ported() {
        // RD5: the Electron launcher's 1.4.4 text is not in the language files; the refusal of a
        // dauntless-revived:// invite and the saved-server notice are (RD22).
        for language in Language::ALL {
            for key in [
                "revived_download_title",
                "dialog_revived_folder",
                "err_windows_runtime_missing",
            ] {
                assert!(language.lookup(key).is_none(), "{} {key}", language.code());
            }
            for key in ["invite_err_revived", "server_revived_disabled"] {
                assert!(language.lookup(key).is_some(), "{} {key}", language.code());
            }
        }
    }
}
