// The isolation hook (isolation/hook.js, MIGRATION_PLAN.md L9, RA2; DR's test/isolation-hook.test.ts): only
// the backend's commands, with arguments of the right shape and size, reach Rust; anything else never
// settles.

import { test } from "node:test";
import assert from "node:assert/strict";
import { loadHook } from "./hook.ts";

const { hook, commands } = loadHook();

function payload(cmd: unknown, args: unknown) {
  return { cmd, callback: 1, error: 2, payload: args, options: undefined };
}

function passes(cmd: unknown, args: unknown): boolean {
  const p = payload(cmd, args);
  return hook(p) === p;
}

test("every listed command passes with arguments of the right shape", () => {
  const good: Record<string, unknown> = {
    text: "a".repeat(2048),
    acceptNewCertificate: false,
    carryAccount: true,
    id: "0123456789abcdef01234567",
    username: "a".repeat(32),
    gamePath: "C:\\Games\\Dauntless",
    index: 15,
    patch: { graphics: -1, language: "pt-BR", hostTools: true, runtimeProfile: "development", streamerMode: false },
    on: true,
    target: "dumper7_zip",
    action: "publish_runtime",
    folder: "gamedata_packs",
    request: { name: "Friends", mode: "public", publicHost: "play.example.org" },
    forName: "Sam",
    multipliers: { xp: 1, rewards: 100 },
    onProgress: "__CHANNEL__:123",
  };
  assert.equal(Object.keys(commands).length, 52);
  for (const [cmd, spec] of Object.entries(commands)) {
    const args: Record<string, unknown> = {};
    for (const name of Object.keys(spec)) {
      assert.ok(name in good, `${cmd} ${name}`);
      args[name] = name === "text" && cmd !== "check_invite" && cmd !== "submit_invite" ? "a" : good[name];
    }
    assert.ok(passes(cmd, args), cmd);
  }
  assert.ok(passes("plugin:event|listen", { event: "snapshot", target: { kind: "Any" }, handler: 7 }));
  assert.ok(passes("plugin:window|start_dragging", undefined));
  assert.ok(passes("plugin:window|minimize", { label: "main" }));
});

test("a forged command name never reaches the backend", () => {
  for (const cmd of [
    "plugin:fs|read_file",
    "plugin:shell|execute",
    "plugin:opener|open_url",
    "plugin:clipboard-manager|write_text",
    "revived_install",
    "start_install",
    "start_offline",
    "get_snapshot ",
    "__proto__",
    "constructor",
    "toString",
    7,
    null,
  ]) {
    const p = payload(cmd, {});
    const result = hook(p);
    // The hook's own pending promise (from the hook's realm, so not instanceof this realm's Promise)
    assert.notEqual(result, p, String(cmd));
    assert.equal(typeof (result as { then?: unknown }).then, "function", String(cmd));
  }
});

test("a 1 MB string or a wrong argument never reaches the backend", () => {
  const big = "x".repeat(1024 * 1024);
  assert.equal(passes("check_invite", { text: big }), false);
  assert.equal(passes("register", { username: "a".repeat(33) }), false);
  assert.equal(passes("register", { username: "a".repeat(32) }), true);
  assert.equal(passes("register", {}), false, "missing");
  assert.equal(passes("register", { username: 7 }), false, "not a string");
  assert.equal(passes("register", { username: "a", extra: 1 }), false, "unknown argument");
  assert.equal(passes("submit_invite", { text: "x", acceptNewCertificate: "yes", carryAccount: false }), false);
  assert.equal(passes("submit_invite", { text: "x", acceptNewCertificate: true }), false, "carryAccount missing");
  assert.equal(passes("set_settings", { patch: { unexpected: true } }), false);
  assert.equal(passes("set_settings", { patch: { huntRegion: "us" } }), false, "DR's settings are gone");
  assert.equal(passes("set_settings", { patch: { language: big } }), false);
  assert.equal(passes("set_settings", { patch: [] }), false);
  assert.equal(passes("subscribe_progress", { onProgress: "https://example.com" }), false);
  assert.equal(passes("plugin:event|listen", { event: big, handler: 1 }), false);
  assert.equal(passes("get_snapshot", []), false);
  assert.equal(passes("copy_text", { text: "x".repeat(4097) }), false);
  assert.equal(passes("copy_text", { text: "x".repeat(4096) }), true);
});

test("an index is an integer from 0 to its limit", () => {
  for (const index of [0, 1, 15]) assert.equal(passes("use_detected_game", { index }), true, String(index));
  for (const index of [16, -1, 1.5, NaN, Infinity, "1", null, [1]]) {
    assert.equal(passes("use_detected_game", { index }), false, String(index));
  }
  assert.equal(passes("use_detected_game", {}), false);
});

test("a name is one of its list, exactly", () => {
  assert.equal(passes("open_external", { target: "project_source" }), true);
  assert.equal(passes("host_action", { action: "inject_dumper" }), true);
  assert.equal(passes("host_open", { folder: "workspace" }), true);
  for (const target of ["https://example.com", "Project_source", "project_source ", "discord", "__proto__", "", 1, null]) {
    assert.equal(passes("open_external", { target }), false, String(target));
  }
  assert.equal(passes("host_action", { action: "rm -rf" }), false);
  assert.equal(passes("host_open", { folder: "C:\\Windows" }), false);
});

test("an object has exactly its fields, each of the right shape", () => {
  const setup = (request: unknown) => passes("host_setup", { request });
  assert.equal(setup({ name: "Friends", mode: "private", publicHost: null }), true);
  assert.equal(setup({ name: "Friends", mode: "private" }), true, "an optional field may be absent");
  assert.equal(setup({ name: "Friends", mode: "public", publicHost: "a".repeat(253) }), true);
  assert.equal(setup({ name: "Friends", mode: "public", publicHost: "a".repeat(254) }), false);
  assert.equal(setup({ name: "a".repeat(61), mode: "private", publicHost: null }), false);
  assert.equal(setup({ name: "Friends", mode: "lan", publicHost: null }), false);
  assert.equal(setup({ name: "Friends", mode: "private", publicHost: null, gameRoot: "C:\\" }), false, "an unknown field");
  assert.equal(setup({ mode: "private", publicHost: null }), false, "a missing field");
  assert.equal(setup(null), false, "not nullable");
  assert.equal(setup([]), false);
  assert.equal(setup("Friends"), false);

  const multipliers = (value: unknown) => passes("host_set_multipliers", { multipliers: value });
  assert.equal(multipliers(null), true, "off");
  assert.equal(multipliers({ xp: 1, rewards: 1 }), true);
  assert.equal(multipliers({ xp: 2.5, rewards: 100 }), true);
  for (const bad of [{ xp: 0, rewards: 1 }, { xp: 101, rewards: 1 }, { xp: NaN, rewards: 1 }, { xp: "2", rewards: 1 }, { xp: 2 }, {}, [], "2"]) {
    assert.equal(multipliers(bad), false, JSON.stringify(bad));
  }
  assert.equal(passes("host_set_multipliers", {}), false, "the argument itself must be there");
});

test("who an invite is for is optional text", () => {
  assert.equal(passes("host_create_invite", { forName: null }), true);
  assert.equal(passes("host_create_invite", {}), true);
  assert.equal(passes("host_create_invite", { forName: "a".repeat(40) }), true);
  assert.equal(passes("host_create_invite", { forName: "a".repeat(41) }), false);
  assert.equal(passes("host_create_invite", { forName: 7 }), false);
});

test("a dropped call stays pending", async () => {
  const result = hook(payload("plugin:fs|read_file", {})) as Promise<unknown>;
  const settled = await Promise.race([result.then(() => true), new Promise((resolve) => setTimeout(() => resolve(false), 20))]);
  assert.equal(settled, false);
});
