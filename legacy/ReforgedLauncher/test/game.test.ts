// Finding the player's Dauntless 1.14.7: the folder layouts, Version.txt and the store manifests.

import { test, after } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  cleanGameFolderInput,
  gameRootCandidates,
  isSupportedVersionText,
  locateExistingGame,
  parseEpicItem,
  parseLegendaryInstalled,
  readGameVersion,
} from "../src/main/game-folder";
import { GAME_VERSION_TEXT } from "../src/main/constants";

const dirs: string[] = [];
after(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});

function fakeGame(version: string | null = GAME_VERSION_TEXT): string {
  const base = mkdtempSync(path.join(tmpdir(), "rr-game-"));
  dirs.push(base);
  const root = path.join(base, "Dauntless");
  mkdirSync(path.join(root, "Archon", "Binaries", "Win64"), { recursive: true });
  writeFileSync(path.join(root, "Archon", "Binaries", "Win64", "Dauntless-Win64-Shipping.exe"), "MZ fake");
  if (version !== null) writeFileSync(path.join(root, "Version.txt"), `\uFEFF${version}\r\n`);
  return root;
}

test("cleanGameFolderInput: drive-letter paths only on Windows, quotes from Copy as path removed", () => {
  assert.equal(cleanGameFolderInput('"C:\\Games\\Dauntless"', "win32"), "C:\\Games\\Dauntless");
  assert.equal(cleanGameFolderInput("C:/Games/Dauntless", "win32"), "C:\\Games\\Dauntless");
  assert.equal(cleanGameFolderInput("\\\\server\\share\\Dauntless", "win32"), null, "UNC paths are never touched");
  assert.equal(cleanGameFolderInput("\\\\?\\C:\\Games", "win32"), null);
  assert.equal(cleanGameFolderInput("\\Games", "win32"), null);
  assert.equal(cleanGameFolderInput("Games\\Dauntless", "win32"), null);
  assert.equal(cleanGameFolderInput("", "win32"), null);
  assert.equal(cleanGameFolderInput("C:\\a\0b", "win32"), null);
  assert.equal(cleanGameFolderInput("/home/me/Dauntless", "linux"), "/home/me/Dauntless");
  assert.equal(cleanGameFolderInput("relative/path", "linux"), null);
});

test("gameRootCandidates: the root, a Dauntless subfolder, Archon's parent, three up from Win64 or the exe", () => {
  assert.deepEqual(gameRootCandidates("C:\\Games\\Dauntless", "win32"), ["C:\\Games\\Dauntless", "C:\\Games\\Dauntless\\Dauntless"]);
  assert.ok(gameRootCandidates("C:\\Games\\Dauntless\\Archon", "win32").includes("C:\\Games\\Dauntless"));
  assert.ok(gameRootCandidates("C:\\Games\\Dauntless\\Archon\\Binaries\\Win64", "win32").includes("C:\\Games\\Dauntless"));
  assert.ok(gameRootCandidates("C:\\Games\\Dauntless\\Archon\\Binaries\\Win64\\Dauntless-Win64-Shipping.exe", "win32").includes("C:\\Games\\Dauntless"));
});

test("locateExistingGame finds the game from any of the layouts", async () => {
  const root = fakeGame();
  for (const input of [root, path.dirname(root), path.join(root, "Archon"), path.join(root, "Archon", "Binaries", "Win64")]) {
    assert.deepEqual(await locateExistingGame(input, process.platform), { ok: true, root }, input);
  }
  assert.deepEqual(await locateExistingGame(path.join(root, "nothing-here"), process.platform), { ok: false, reason: "not_found" });
  assert.deepEqual(await locateExistingGame("not absolute", process.platform), { ok: false, reason: "invalid_path" });
});

test("Version.txt: 1.14.7 (CL 647472) is accepted, other builds are not", async () => {
  assert.equal(isSupportedVersionText(GAME_VERSION_TEXT), true);
  assert.equal(isSupportedVersionText("dauntless_rel-1.14.7_Shipping_2024-04-02_10-00-00_647472"), true);
  assert.equal(isSupportedVersionText("dauntless_rel-1.4.4_Shipping_2020-10-28_20-11-15_239827"), false);
  assert.equal(isSupportedVersionText("dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_6474720"), false);
  assert.equal(isSupportedVersionText("dauntless_rel-1.14.6_Shipping_2024-03-01_647472"), false);
  assert.deepEqual(await readGameVersion(fakeGame()), { text: GAME_VERSION_TEXT, ok: true });
  assert.deepEqual(await readGameVersion(fakeGame("dauntless_rel-1.12.0_Shipping_x_600000")), { text: "dauntless_rel-1.12.0_Shipping_x_600000", ok: false });
  assert.deepEqual(await readGameVersion(fakeGame(null)), { text: null, ok: false });
  assert.deepEqual(await readGameVersion(fakeGame("bad\u0007text")), { text: null, ok: false });
});

test("store manifests: Legendary's installed.json and the Epic Games Launcher's .item files", () => {
  const installed = {
    Jackal: { app_name: "Jackal", install_path: "D:\\Games\\Dauntless", version: "1.14.7" },
    Other: { app_name: "Fortnite", install_path: "D:\\Games\\Fortnite" },
    Broken: { app_name: "Jackal", install_path: 7 },
  };
  assert.deepEqual(parseLegendaryInstalled(installed, "legendary"), [{ root: "D:\\Games\\Dauntless", source: "legendary", version: "1.14.7" }]);
  assert.deepEqual(parseLegendaryInstalled(null, "heroic"), []);
  assert.deepEqual(parseEpicItem({ AppName: "Jackal", InstallLocation: "C:\\Program Files\\Epic Games\\Dauntless", AppVersionString: "1.14.7" }), {
    root: "C:\\Program Files\\Epic Games\\Dauntless",
    source: "epic",
    version: "1.14.7",
  });
  assert.equal(parseEpicItem({ AppName: "Fortnite", InstallLocation: "C:\\x" }), null);
  assert.equal(parseEpicItem({ AppName: "Jackal" }), null);
});
