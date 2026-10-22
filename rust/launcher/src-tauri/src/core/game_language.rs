//! The game's own text language (row L16): `gameLocale` in `src/main/launch.ts`. The 1.4.4 client has
//! text for exactly these cultures (`Archon/Content/Localization/Game/<culture>`); the setting is sent to
//! it as `-epiclocale`.
#![cfg_attr(not(test), allow(dead_code, reason = "the game launch (L31) sends it"))]

use crate::bridge::types::GameLanguage;

/// In the order "auto" looks through them.
pub const GAME_LOCALES: [&str; 8] = ["en-US", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "pt-BR", "ru-RU"];

impl GameLanguage {
    /// The culture name, or None for "auto".
    pub fn locale(self) -> Option<&'static str> {
        Some(match self {
            Self::Auto => return None,
            Self::EnUs => "en-US",
            Self::DeDe => "de-DE",
            Self::EsEs => "es-ES",
            Self::FrFr => "fr-FR",
            Self::ItIt => "it-IT",
            Self::JaJp => "ja-JP",
            Self::PtBr => "pt-BR",
            Self::RuRu => "ru-RU",
        })
    }
}

/// The `-epiclocale` for the Game language setting. "auto" takes the operating system's language when
/// the game has text for it (es-419 -> es-ES, pt-PT -> pt-BR), and English otherwise.
pub fn game_locale(setting: GameLanguage, system_locale: &str) -> &'static str {
    if let Some(locale) = setting.locale() {
        return locale;
    }
    let code: String = system_locale.chars().take(2).flat_map(char::to_lowercase).collect();
    GAME_LOCALES
        .into_iter()
        .find(|locale| locale.to_lowercase().starts_with(&format!("{code}-")))
        .unwrap_or("en-US")
}

/// Never an argument the game has no text for.
pub fn launch_locale(locale: Option<&str>) -> &'static str {
    locale
        .and_then(|locale| GAME_LOCALES.into_iter().find(|known| *known == locale))
        .unwrap_or("en-US")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_the_system_when_the_game_has_that_language() {
        assert_eq!(launch_locale(None), "en-US");
        assert_eq!(launch_locale(Some("es-ES")), "es-ES");
        assert_eq!(launch_locale(Some("pt-BR")), "pt-BR");
        assert_eq!(launch_locale(Some("xx-XX")), "en-US");
        assert_eq!(game_locale(GameLanguage::Auto, "es-ES"), "es-ES");
        assert_eq!(game_locale(GameLanguage::Auto, "es-419"), "es-ES");
        assert_eq!(game_locale(GameLanguage::Auto, "de"), "de-DE");
        assert_eq!(game_locale(GameLanguage::Auto, "pt-PT"), "pt-BR");
        assert_eq!(
            game_locale(GameLanguage::Auto, "fi-FI"),
            "en-US",
            "no Finnish text in the game"
        );
        assert_eq!(game_locale(GameLanguage::Auto, ""), "en-US");
        assert_eq!(
            game_locale(GameLanguage::JaJp, "en-US"),
            "ja-JP",
            "a chosen language wins over the system"
        );
    }

    #[test]
    fn every_setting_but_auto_is_a_game_locale() {
        let all = [
            GameLanguage::EnUs,
            GameLanguage::DeDe,
            GameLanguage::EsEs,
            GameLanguage::FrFr,
            GameLanguage::ItIt,
            GameLanguage::JaJp,
            GameLanguage::PtBr,
            GameLanguage::RuRu,
        ];
        for setting in all {
            let locale = setting.locale().unwrap();
            assert!(GAME_LOCALES.contains(&locale));
            assert_eq!(serde_json::to_value(setting).unwrap(), serde_json::Value::from(locale));
        }
        assert_eq!(all.len(), GAME_LOCALES.len());
    }
}
