// The launcher's text, exported for the Tauri backend (RA3, as Dauntless Revived's L14 did):
// rust/launcher/src-tauri/build.rs makes the Rust Language enum and the backend's own text (dialogs, the
// close confirmation) from src-tauri/gen/strings.json. Only the language files are exported, not the
// Electron launcher's 1.4.4 text. This test fails when the export is stale;
// RR_UPDATE_GENERATED=1 npm test -- strings-export rewrites it.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { REGISTRY } from "../src/shared/i18n";

const ROOT = path.resolve(__dirname, "..", "..");
const file = path.join(ROOT, "..", "..", "rust", "launcher", "src-tauri", "gen", "strings.json");

test("rust/launcher/src-tauri/gen/strings.json holds every language file's text", () => {
  const exported = JSON.stringify(REGISTRY, null, 2) + "\n";
  if (process.env.RR_UPDATE_GENERATED === "1") writeFileSync(file, exported);
  let current = "";
  try {
    current = readFileSync(file, "utf8").replace(/\r\n/g, "\n");
  } catch {
    // missing: stale
  }
  assert.ok(current === exported, "stale: run RR_UPDATE_GENERATED=1 npm test -- strings-export");
});
