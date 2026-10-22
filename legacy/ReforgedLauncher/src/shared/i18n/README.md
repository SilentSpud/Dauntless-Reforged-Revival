# Launcher text

Every piece of text the launcher shows, one file per language (RD14). `en.ts` is the source: every other
language has exactly its keys and the same `{placeholders}`, and no text is empty (`test/i18n.test.ts`
checks all three). A launcher row that needs a new string appends it at the end of every language file.

`revived.ts` is not a language: it holds the Electron launcher's text for Dauntless 1.4.4
(`src/main/revived/`), which the Tauri launcher doesn't play (RD5). It is not in the registry, so a new
language doesn't need it (its 1.4.4 text falls back to English), and it is deleted with the Electron
launcher (RA16).

## Adding a language

1. Copy `en.ts` to `<code>.ts`, where `<code>` is the code the settings store (`en`, `pt-BR`). Rename the
   object, type it `Record<LanguageKey, string>` as `pt-BR.ts` does, and translate the text. Keys,
   `{placeholders}` and the brand names stay as they are.
2. Add it to `REGISTRY` in `index.ts`, where you want it in the settings menu.
3. Run `RR_UPDATE_GENERATED=1 npm test -- strings-export`: it rewrites
   `rust/launcher/src-tauri/gen/strings.json`, from which the Tauri backend makes its `Language` and its
   own text (dialogs, the close confirmation), so the backend gets the language by rebuilding.

Until the page is ported (RA13) it still names its languages itself: add the code to `Language` and
`LANGUAGES` in `../types.ts`, and its name and time locale to `LANGUAGE_NAMES` and `TIME_LOCALES` in
`../../renderer/index.ts`.

`systemLanguage` needs nothing: a first start picks the language whose code matches the system's locale,
or one for the same language (any Portuguese locale gives `pt-BR`), else English.

The game's own text languages are a separate, fixed list (`GAME_LANGUAGES` in `../types.ts`). A new
launcher language does not change it.
