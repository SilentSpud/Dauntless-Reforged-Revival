// The game's command line and environment, and the config files written before each launch.

import { test, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildLaunchArgs, describeLaunch, gameLocale, launchEnvironment, maskArgs } from "../src/main/launch";
import { applyGameConfig, decodeIni, PHOENIX_ENDPOINTS_SECTION, removeIniSection, rewriteEngineIniText, rewriteGameUserSettingsText, xmppLines } from "../src/main/engineini";
import { redact } from "../src/main/log";

const KEY = "RRK_" + "c".repeat(48);
const dirs: string[] = [];
after(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});

test("launch arguments: the 1.14.7 command line with the key as the exchange code", () => {
  const args = buildLaunchArgs({ key: KEY, userId: "0b7c2f6e-1d2a-4c55-9d6f-7a1e2b3c4d5e", username: "Slayer_42", windowed: false, gameLocale: "pt-BR" });
  assert.deepEqual(args.slice(0, 6), ["-DisableEOS", "-EpicPortal", "-NoEAC", "-AUTH_TYPE=exchangecode", "-AUTH_LOGIN=0b7c2f6e-1d2a-4c55-9d6f-7a1e2b3c4d5e", `-AUTH_PASSWORD=${KEY}`]);
  assert.ok(args.includes("-epicapp=Archon"));
  assert.ok(args.includes("-epicsandboxid=jackal"));
  assert.ok(args.includes("-epicusername=Slayer_42"));
  assert.ok(args.includes("-epiclocale=pt-BR"));
  assert.ok(!args.includes("-windowed"));
  const windowed = buildLaunchArgs({ key: KEY, userId: "u1", username: "abc", windowed: true, gameLocale: "xx-YY" });
  assert.ok(windowed.includes("-windowed") && windowed.includes("-ResX=1280") && windowed.includes("-ResY=720"));
  assert.ok(windowed.includes("-epiclocale=en-US"), "an unknown locale falls back to English");
  assert.throws(() => buildLaunchArgs({ key: "nope", userId: "u1", username: "abc", windowed: false }));
  assert.throws(() => buildLaunchArgs({ key: KEY, userId: "a b", username: "abc", windowed: false }));
  assert.throws(() => buildLaunchArgs({ key: KEY, userId: "u1", username: "a\"b", windowed: false }));
});

test("the key never reaches a log line", () => {
  const args = buildLaunchArgs({ key: KEY, userId: "u1", username: "abc", windowed: false });
  assert.ok(maskArgs(args).includes("-AUTH_PASSWORD=<hidden>"));
  assert.ok(!describeLaunch("game.exe", args).includes(KEY));
  assert.ok(!redact(`calling with ${KEY} now`).includes(KEY));
  assert.ok(!redact(`x-reforged-user-api-key: ${KEY}`).includes(KEY));
});

test("gameLocale: auto follows the system when the game has that language", () => {
  assert.equal(gameLocale("auto", "pt-BR"), "pt-BR");
  assert.equal(gameLocale("auto", "pt-PT"), "pt-BR");
  assert.equal(gameLocale("auto", "es-419"), "es-ES");
  assert.equal(gameLocale("auto", "fi-FI"), "en-US");
  assert.equal(gameLocale("de-DE", "pt-BR"), "de-DE");
});

test("launchEnvironment: the runtime is pointed at the local relay and nothing else", () => {
  const env = launchEnvironment({ PATH: "x", REFORGED_BACKEND_URL: "https://evil.example", MYSTICPARADOX_LOG: "1", reforged_other: "y" }, "http://127.0.0.1:61100", "development");
  assert.equal(env.REFORGED_BACKEND_URL, "http://127.0.0.1:61100");
  assert.equal(env.MYSTICPARADOX_RUNTIME_PROFILE, "development");
  assert.equal(env.PATH, "x");
  assert.equal(env.MYSTICPARADOX_LOG, undefined);
  assert.equal(env.reforged_other, undefined);
  assert.throws(() => launchEnvironment({}, "http://192.168.1.2:61100", "production"));
  assert.throws(() => launchEnvironment({}, "https://127.0.0.1:61100", "production"));
});

test("Engine.ini: chat goes to the relay, the owned sections are replaced and everything else is kept", () => {
  const existing = ["[Core.Log]", "LogOnline=Verbose", "", "[OnlineSubsystemMcp.XMPP]", 'ServerAddr="wss://xmpp-service-prod.ol.epicgames.com"', "ServerPort=443", "bUseSSL=true", "", "[SystemSettings]", "sg.ShadowQuality=0", "[/Script/Engine.Engine]", "bSmoothFrameRate=true"];
  const out = rewriteEngineIniText(existing, "127.0.0.1", 61100, -1);
  assert.deepEqual(out.slice(0, 4), ['[OnlineSubsystemMcp.XMPP]', 'ServerAddr="ws://127.0.0.1"', "ServerPort=61100", "bUseSSL=false"]);
  assert.ok(!out.some((l) => l.includes("epicgames")));
  assert.ok(!out.includes("[SystemSettings]"), "graphics left to the game");
  assert.ok(out.includes("LogOnline=Verbose") && out.includes("bSmoothFrameRate=true"));
  const forced = rewriteEngineIniText(existing, "127.0.0.1", 61100, 2);
  assert.equal(forced[0], "[SystemSettings]");
  assert.ok(forced.includes("sg.ShadowQuality=2"));
  assert.throws(() => xmppLines("bad host!", 1));
  assert.deepEqual(rewriteGameUserSettingsText(["sg.TextureQuality=0", "Other=1"], 3), ["sg.TextureQuality=3", "Other=1"]);
});

test("applyGameConfig writes Engine.ini and keeps a UTF-16 file readable", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "rr-ini-"));
  dirs.push(dir);
  writeFileSync(path.join(dir, "Engine.ini"), Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from("[Custom]\r\nName=Jörð\r\n", "utf16le")]));
  const r = await applyGameConfig({ host: "127.0.0.1", port: 61100, graphics: -1, configDir: dir });
  const text = decodeIni(readFileSync(r.engineIni));
  assert.ok(text.includes("ServerPort=61100"));
  assert.ok(text.includes("Name=Jörð"));
  assert.equal(r.phoenixEndpointsRemoved, 0);
});

// What Dauntless Revived's launcher leaves in Game.ini (local-server mode), between player sections.
const REVIVED_GAME_INI = [
  "[/Script/Archon.ArchonGameSettings]",
  "bShowHints=False",
  "",
  "[OnlineSubsystemPhoenix]",
  'AuthEndpoint="http://127.0.0.1:61000/auth"',
  'CheckCrossPlayProgressionEndpoint="http://127.0.0.1:61000/features/platform/{platform}"',
  "; a comment",
  'PhoenixStatusMessageEndpoint="http://127.0.0.1:61000/dauntless-status"',
  "",
  "[Custom]",
  "Name=Kept",
];

test("removeIniSection drops one section (any case), counts its keys and keeps the rest", () => {
  const r = removeIniSection(REVIVED_GAME_INI, "onlinesubsystemphoenix");
  assert.equal(r.removed, 3);
  assert.deepEqual(r.lines, ["[/Script/Archon.ArchonGameSettings]", "bShowHints=False", "", "[Custom]", "Name=Kept"]);
  assert.deepEqual(removeIniSection(["[A]", "x=1"], PHOENIX_ENDPOINTS_SECTION), { lines: ["[A]", "x=1"], removed: 0 });
});

test("applyGameConfig (1.14.7): Revived's 127.0.0.1 endpoints leave Game.ini, with a copy; nothing else changes", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "rr-ini-"));
  dirs.push(dir);
  const gameIni = path.join(dir, "Game.ini");
  writeFileSync(gameIni, REVIVED_GAME_INI.map((l) => l + "\r\n").join(""));
  // Dauntless 1.4.4 (Revived servers) keeps them.
  const kept = await applyGameConfig({ host: "127.0.0.1", port: 61100, graphics: -1, configDir: dir, memory: true });
  assert.equal(kept.phoenixEndpointsRemoved, 0);
  assert.match(decodeIni(readFileSync(gameIni)), /CheckCrossPlayProgressionEndpoint/);

  const r = await applyGameConfig({ host: "127.0.0.1", port: 61100, graphics: -1, configDir: dir, clearPhoenixEndpoints: true });
  assert.equal(r.phoenixEndpointsRemoved, 3);
  const text = decodeIni(readFileSync(gameIni));
  assert.ok(!text.includes("127.0.0.1"), "no endpoint left");
  assert.ok(text.includes("bShowHints=False") && text.includes("Name=Kept"));
  assert.match(decodeIni(readFileSync(`${gameIni}.before-reforged`)), /CheckCrossPlayProgressionEndpoint/);

  // Already clean: not written again, and no Game.ini is made where there is none.
  const again = await applyGameConfig({ host: "127.0.0.1", port: 61100, graphics: -1, configDir: dir, clearPhoenixEndpoints: true });
  assert.equal(again.phoenixEndpointsRemoved, 0);
  const empty = mkdtempSync(path.join(tmpdir(), "rr-ini-"));
  dirs.push(empty);
  await applyGameConfig({ host: "127.0.0.1", port: 61100, graphics: -1, configDir: empty, clearPhoenixEndpoints: true });
  assert.ok(!existsSync(path.join(empty, "Game.ini")));
});
