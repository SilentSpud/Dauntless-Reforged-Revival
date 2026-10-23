// Every language file has the same keys and the same {placeholders}, and every key the page builds
// from a code (an error, a notice, a step state...) exists.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { isStringKey, REGISTRY, STRINGS, systemLanguage, translate, type LanguageKey, type StringKey } from "../src/shared/i18n";
import { REVIVED } from "../src/shared/i18n/revived";
import { LANGUAGES } from "../src/shared/types";

const ROOT = path.resolve(__dirname, "..", "..");

function placeholders(s: string): string[] {
  return [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
}

test("every language has exactly the English keys, with the same placeholders, and no empty text", () => {
  const en = STRINGS.en;
  for (const lang of LANGUAGES) {
    const table = STRINGS[lang];
    assert.deepEqual(Object.keys(table).sort(), Object.keys(en).sort(), lang);
    for (const k of Object.keys(en) as StringKey[]) {
      assert.ok(table[k].trim().length > 0, `${lang}.${k} is empty`);
      assert.deepEqual(placeholders(table[k]), placeholders(en[k]), `${lang}.${k} placeholders`);
    }
  }
});

test("every language file has exactly the keys of en.ts", () => {
  const keys = Object.keys(REGISTRY.en).sort();
  assert.deepEqual(Object.keys(REGISTRY).sort(), [...LANGUAGES].sort());
  for (const lang of LANGUAGES) assert.deepEqual(Object.keys(REGISTRY[lang]).sort(), keys, lang);
});

test("the 1.4.4 text is only the Electron launcher's, outside the language files", () => {
  for (const k of Object.keys(REVIVED.en)) assert.equal(Object.prototype.hasOwnProperty.call(REGISTRY.en, k), false, k);
  for (const k of ["revived_download_title", "dialog_revived_folder", "err_windows_runtime_missing", "game_source_revived"]) assert.ok(k in REVIVED.en, k);
});

test("a dauntless-revived:// invite and a saved 1.4.4 server have their text (RD5, RD22)", () => {
  for (const lang of LANGUAGES) {
    for (const k of ["invite_err_revived", "server_revived_disabled"] as LanguageKey[]) assert.ok(REGISTRY[lang][k].includes("1.4.4"), `${lang}.${k}`);
  }
});

test("the Portuguese table is really translated", () => {
  const same = (Object.keys(STRINGS.en) as StringKey[]).filter((k) => STRINGS.en[k] === STRINGS["pt-BR"][k]);
  // Names (Ramsgate, Legendary, Dauntless 1.14.7...) may stay the same; prose may not.
  for (const k of same) assert.ok(STRINGS.en[k].split(" ").length <= 4, `pt-BR.${k} is still English: ${STRINGS.en[k]}`);
});

test("translate fills placeholders; systemLanguage maps any Portuguese locale to pt-BR", () => {
  assert.equal(translate("en", "ready_title", { user: "Ana" }), "Welcome back, Ana");
  assert.equal(translate("pt-BR", "ready_title", { user: "Ana" }), "Bem-vindo de volta, Ana");
  assert.equal(systemLanguage("pt-BR"), "pt-BR");
  assert.equal(systemLanguage("pt-PT"), "pt-BR");
  assert.equal(systemLanguage("pt"), "pt-BR");
  assert.equal(systemLanguage("en-US"), "en");
  assert.equal(systemLanguage("pt_BR"), "pt-BR");
  assert.equal(systemLanguage("PT-br"), "pt-BR");
  assert.equal(systemLanguage("ptx"), "en");
  assert.equal(systemLanguage("en"), "en");
  assert.equal(systemLanguage(""), "en");
  assert.equal(systemLanguage("fi"), "en");
  assert.equal(isStringKey("btn_play"), true);
  assert.equal(isStringKey("__proto__"), false);
});

// The prefixes the page and the main process build keys from, with every value the code can produce.
const FAMILIES: Record<string, readonly string[]> = {
  err_: ["account_banned", "invite_invalid_format", "invite_invalid", "username_invalid", "username_taken", "registration_closed", "bad_request", "server_unreachable", "server_error", "key_invalid", "key_rejected", "key_storage_unavailable", "folder_invalid", "game_folder_not_found", "game_wrong_version", "runtime_unavailable", "runtime_untrusted", "runtime_failed", "runtime_different_on_server", "runtime_in_use", "local_runtime_invalid", "download_failed", "disk_space", "already_running", "launch_failed", "config_failed", "busy", "cancelled", "cert_mismatch", "cert_changed", "rate_limited", "relay_port_busy", "relay_failed", "host_failed", "unknown"],
  invite_err_: ["empty", "too_long", "not_invite", "revived", "version", "params", "mode", "host", "port", "fp", "code", "name", "share"],
  reg_check_: ["empty", "too_short", "too_long", "bad_chars", "ok"],
  notice_: ["keep_open", "backup_saved", "runtime_installed", "runtime_installed_restart", "game_verified", "sdk_imported", "host_done"],
  where_: ["menu", "city", "hunt", "dojo", "tutorial", "unknown"],
  kind_: ["city", "hunt", "dojo", "tutorial"],
  gfx_: ["menu", "0", "1", "2", "3", "4"],
  profile_: ["production", "development"],
  game_source_: ["epic", "legendary", "heroic", "chosen"],
  reg_mode_: ["OPEN", "INVITECODE", "NONE"],
  gamedata_kind_: ["real", "synthetic", "missing", "unknown"],
  runtime_state_: ["ok", "missing", "outdated", "untrusted", "unavailable", "local"],
  host_job_: ["inject", "build_runtime", "build_loader", "build_exporter", "import_sdk", "copy_exports", "synthetic_data", "import_gamedata", "publish_runtime", "setup", "generate_gamedata", "export_gamedata", "create_invite"],
  host_job_state_: ["running", "done", "failed"],
  host_err_: ["workspace", "dumper", "folder", "no_dump", "msbuild", "cargo", "no_exports", "windows_only", "game_not_running", "exporter", "no_process", "node_missing", "npm_missing", "not_configured", "gamedata_missing", "server_running", "server_not_running", "setup_form", "invite"],
  host_tab_: ["server", "gamedata", "build", "requirements"],
  hsrv_state_: ["stopped", "starting", "running", "stopping", "external", "failed"],
  hsrv_text_: ["stopped", "starting", "running", "stopping", "external"],
  hsrv_mode_: ["private", "public"],
  hsrv_mode_short_: ["private", "public"],
  hreq_state_: ["ok", "missing", "problem"],
  set_tab_: ["game", "graphics", "account", "launcher"],
  credits_role_: ["maintainer", "creator", "contributor"],
  step_: ["join", "register", "game", "runtime", "play"],
};

// The Requirements tab's guides: a name, why it is needed and its install steps (src/renderer/index.ts).
const REQUIREMENT_STEPS: Record<string, number> = { dauntless: 2, git: 3, node: 3, mongodb: 5, tailscale: 3, visual_studio: 4, rust: 4, dumper7: 3, vc_redist: 2 };

test("every requirement has its name, its reason and each install step", () => {
  for (const [id, steps] of Object.entries(REQUIREMENT_STEPS)) {
    for (const part of ["name", "why", ...Array.from({ length: steps }, (_, i) => `s${i + 1}`)]) assert.ok(isStringKey(`req_${id}_${part}`), `missing key req_${id}_${part}`);
    assert.equal(isStringKey(`req_${id}_s${steps + 1}`), false, `req_${id} has more steps than the page shows`);
  }
});

test("every key built from a code exists", () => {
  for (const [prefix, values] of Object.entries(FAMILIES)) {
    for (const v of values) assert.ok(isStringKey(prefix + v), `missing key ${prefix}${v}`);
  }
});

test("every literal key in the page, the main process and index.html exists", () => {
  const files = ["src/renderer/index.ts", "src/main.ts", "src/shared/ui-model.ts", "index.html"].map((f) => path.join(ROOT, f));
  const missing: string[] = [];
  for (const file of files) {
    const text = readFileSync(file, "utf8");
    const keys = [
      ...[...text.matchAll(/\b(?:t|translate\([^,]+,)\s*\(?\s*"([a-z][a-z0-9_]+)"/g)].map((m) => m[1]),
      ...[...text.matchAll(/data-i18n(?:-aria)?="([a-z0-9_]+)"/g)].map((m) => m[1]),
      ...[...text.matchAll(/(?:labelKey|titleKey|key): "([a-z][a-z0-9_]+)"/g)].map((m) => m[1]),
    ];
    for (const k of keys) if (!isStringKey(k)) missing.push(`${path.basename(file)}: ${k}`);
  }
  assert.deepEqual(missing, []);
});
