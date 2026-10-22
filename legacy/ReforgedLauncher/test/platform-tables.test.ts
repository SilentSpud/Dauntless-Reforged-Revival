// rust/launcher/test/cases: the settings and log tables the Tauri launcher's platform layer runs too
// (MIGRATION_PLAN.md RA8). The Tauri launcher writes this file less the 1.4.4 parts (RD5, RD22):
// revivedRoot and revivedVerified are dropped, and a 1.4.4 server is never the current one.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { sanitizeSettings, type StoredSettings } from "../src/main/settings";
import { addSecret, redact, removeSecret } from "../src/main/log";

const table = (name: string) => JSON.parse(readFileSync(path.join(__dirname, "../../../../rust/launcher/test/cases", name), "utf8"));

function tauriView(s: StoredSettings): Partial<StoredSettings> {
  const { revivedRoot: _root, revivedVerified: _verified, ...rest } = s;
  return rest.server?.game === "revived" ? { ...rest, server: null } : rest;
}

test("the shared settings table, as the Tauri launcher writes the file", () => {
  const cases = table("settings.json") as { platform: "win32" | "linux"; input: unknown; language: "en"; text: string }[];
  assert.ok(cases.length > 100);
  for (const { platform, input, language, text } of cases) {
    assert.equal(JSON.stringify(tauriView(sanitizeSettings(input, language, platform)), null, 2), text, `${platform} ${JSON.stringify(input)?.slice(0, 120)}`);
  }
});

test("the shared redaction table", () => {
  const { secrets, removed, cases } = table("log.json") as { secrets: string[]; removed: string; cases: { line: string; redacted: string }[] };
  for (const secret of secrets) addSecret(secret);
  const last = cases[cases.length - 1];
  for (const { line, redacted } of cases.slice(0, -1)) assert.equal(redact(line), redacted, line);
  removeSecret(removed);
  assert.equal(redact(last.line), last.redacted);
  for (const secret of secrets) removeSecret(secret);
});
