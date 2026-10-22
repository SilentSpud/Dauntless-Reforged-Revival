//! The backend's own text (row L14): the dialogs the backend shows, in the player's language, from the
//! same strings the page uses (`gen/strings.json`, made into `Language` and its tables by build.rs).
//! Nothing here lists the languages: a language added to the page appears by rebuilding.
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
pub const DIALOG_INSTALL_FOLDER: &str = "dialog_install_folder";
pub const DIALOG_EXISTING_FOLDER: &str = "dialog_existing_folder";

#[cfg(test)]
const USED: &[&str] = &[
    APP_TITLE,
    CLOSE_TITLE,
    CLOSE_TEXT,
    CLOSE_KEEP,
    CLOSE_ANYWAY,
    DIALOG_INSTALL_FOLDER,
    DIALOG_EXISTING_FOLDER,
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

/// `systemLanguage`: the language a first start picks from the operating system's locale ("es-ES",
/// "fi", "en-US"...): its first two letters, if the launcher has that language, else English.
pub fn system_language(locale: &str) -> Language {
    let code: String = locale.chars().take(2).flat_map(char::to_lowercase).collect();
    Language::from_code(&code).unwrap_or(Language::En)
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
        assert_eq!(translate(Language::Fi, "window_close", &[]), "Sulje");
        assert_eq!(translate(Language::En, "no_such_key", &[]), "no_such_key");
        assert_eq!(translate(Language::En, "x {a} {a}", &[("a", "1")]), "x 1 1");
        assert_eq!(system_language("es-ES"), Language::Es);
        assert_eq!(system_language("FI"), Language::Fi);
        assert_eq!(system_language("zh-CN"), Language::En);
        assert_eq!(system_language(""), Language::En);
        assert_eq!(Language::from_code("ru"), Some(Language::Ru));
        assert_eq!(Language::ALL[0], Language::En);
    }
}
