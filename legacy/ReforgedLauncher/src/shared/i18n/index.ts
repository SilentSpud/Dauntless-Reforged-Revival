// Every piece of UI text, one file per language (RD14, as Dauntless Revived's L13 did). Keys are shared;
// tests check that every language has exactly the English keys and the same {placeholders}. Based on the
// Dauntless Revived launcher's text, rewritten for Dauntless 1.14.7, its game runtime and the Host tools.
// To add a language, see README.md.

import { LANGUAGES, type Language } from "../types";
import en, { type LanguageKey } from "./en";
import ptBR from "./pt-BR";
import { REVIVED, type RevivedKey } from "./revived";

export type { LanguageKey };
export type StringKey = LanguageKey | RevivedKey;

// The registry: each language's own file, in the order the settings menu lists them. The Tauri backend's
// text is made from this (test/strings-export.test.ts writes src-tauri/gen/strings.json).
export const REGISTRY: Record<Language, Record<LanguageKey, string>> = { en, "pt-BR": ptBR };

// What the page shows: the registry plus the Electron launcher's 1.4.4 text (revived.ts).
export const STRINGS = Object.fromEntries(
  LANGUAGES.map((lang) => [lang, { ...REVIVED.en, ...REVIVED[lang], ...REGISTRY[lang] }]),
) as Record<Language, Record<StringKey, string>>;

export function translate(lang: Language, key: StringKey, vars?: Record<string, string | number>): string {
  const table = STRINGS[lang] ?? STRINGS.en;
  let text = table[key] ?? STRINGS.en[key] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) text = text.split(`{${k}}`).join(String(v));
  }
  return text;
}

export function isStringKey(k: string): k is StringKey {
  return Object.prototype.hasOwnProperty.call(STRINGS.en, k);
}

// The language a first start picks from the operating system's locale ("pt-BR", "pt-PT", "en-US"...): the
// language with that exact code, else the first one for the same language (so any Portuguese locale gets
// Brazilian Portuguese), else English.
export function systemLanguage(locale: string): Language {
  const code = locale.toLowerCase().replace(/_/g, "-");
  const language = code.split("-")[0];
  return (
    LANGUAGES.find((l) => l.toLowerCase() === code) ??
    LANGUAGES.find((l) => l.toLowerCase().split("-")[0] === language) ??
    "en"
  );
}
