// The Host page's server: the host kit's commands and the server run from the launcher, against a fake
// host kit (a small script in place of ReforgedHost's dist/cli.js) on this machine's Node.js.

import { test, after } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { cleanLine, failureLine, findNode, HostServer, hostKitNeedsBuild, SERVER_READY_LINE } from "../src/main/hostserver";
import { findGit, findMongod, nodeVersionOk } from "../src/main/requirements";
import { hostMultipliers, hostSetupRequest, inviteFor } from "../src/main/ipc-validate";
import type { HostServerView } from "../src/shared/types";

const dirs: string[] = [];
after(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});

function temp(): string {
  const d = mkdtempSync(path.join(tmpdir(), "rr-hostserver-"));
  dirs.push(d);
  return d;
}

// A workspace whose host kit is built already: the fake cli answers start, invite and a failing command.
function fakeWorkspace(): string {
  const ws = temp();
  const kit = path.join(ws, "legacy", "ReforgedHost");
  mkdirSync(path.join(kit, "dist"), { recursive: true });
  mkdirSync(path.join(kit, "node_modules"), { recursive: true });
  writeFileSync(path.join(kit, "package.json"), "{}");
  writeFileSync(
    path.join(kit, "dist", "cli.js"),
    `
const cmd = process.argv[2];
if (cmd === "start") {
  if (process.env.REFORGED_CONTROL !== "stdin") { console.error("Error: not controlled"); process.exit(2); }
  console.log("\\u001b[32mmongod   | ready\\u001b[0m");
  console.log("${SERVER_READY_LINE}");
  let rest = "";
  process.stdin.on("data", (d) => { rest += d; if (rest.includes("stop")) { console.log("Stopped."); process.exit(0); } });
  setInterval(() => undefined, 1000);
} else if (cmd === "invite") {
  console.log("noise");
  console.log(JSON.stringify({ link: "reforged-revival://join/abc", for: process.argv[5] ?? null }));
} else {
  console.log("working");
  console.error("\\nError: game data is missing: npm run gamedata");
  process.exit(1);
}
`,
  );
  return ws;
}

function server(opts: { probe?: () => Promise<boolean> } = {}): { s: HostServer; views: HostServerView[] } {
  const views: HostServerView[] = [];
  const s: HostServer = new HostServer({
    env: process.env,
    platform: process.platform,
    findNode: () => ({ node: process.execPath, npmCli: null }),
    probe: opts.probe ?? (async () => false),
    stopTimeoutMs: 10000,
    onChange: () => views.push(s.view()),
  });
  return { s, views };
}

async function until(check: () => boolean, ms = 10000): Promise<void> {
  const end = Date.now() + ms;
  while (!check()) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 25));
  }
}

test("the server starts, says it is up, and stops on request", async () => {
  const ws = fakeWorkspace();
  const { s, views } = server();
  await s.start(ws);
  await until(() => s.serverState === "running");
  assert.ok(s.view().lines.includes("mongod   | ready"), "colours are taken out");
  assert.ok(!s.view().lines.includes(SERVER_READY_LINE), "the ready marker is not shown");
  assert.ok(views.some((v) => v.state === "starting"));
  await s.stop();
  await until(() => s.serverState === "stopped");
  assert.ok(s.view().lines.includes("Stopped."), "it stopped itself, from the stop line");
  assert.equal(s.running, false);
});

test("a server that fails says why, and one started elsewhere is not started again", async () => {
  const ws = fakeWorkspace();
  writeFileSync(path.join(ws, "legacy", "ReforgedHost", "dist", "cli.js"), 'console.error("\\nError: port 27017 is already taken"); process.exit(1);');
  const { s } = server();
  await s.start(ws);
  await until(() => s.serverState === "failed");
  assert.equal(s.view().message, "port 27017 is already taken");

  const other = server({ probe: async () => true }).s;
  await other.start(ws);
  assert.equal(other.serverState, "external");
  assert.equal(other.running, false);
});

test("one-shot commands give their lines and the error", async () => {
  const ws = fakeWorkspace();
  const { s } = server();
  const invite = await s.command(ws, ["invite", "--json", "--for", "Alex"], "invite");
  assert.equal(invite.code, 0);
  assert.deepEqual(JSON.parse(invite.lines[invite.lines.length - 1]), { link: "reforged-revival://join/abc", for: "Alex" });
  const failed = await s.command(ws, ["gamedata"], "gamedata");
  assert.equal(failed.code, 1);
  assert.equal(failed.failure, "game data is missing: npm run gamedata");
  await assert.rejects(s.command(temp(), ["gamedata"], "gamedata"), /workspace/, "not a workspace");
});

test("Node.js, Git and MongoDB are found where Windows installs them", () => {
  const files = new Set(["C:\\Program Files\\nodejs\\node.exe", "C:\\Program Files\\nodejs\\node_modules\\npm\\bin\\npm-cli.js", "C:\\Program Files\\Git\\cmd\\git.exe", "C:\\Program Files\\MongoDB\\Server\\8.0\\bin\\mongod.exe"]);
  const exists = (p: string) => files.has(p);
  const env = { PATH: "C:\\Windows\\System32", ProgramFiles: "C:\\Program Files" };
  assert.deepEqual(findNode(env, exists, "win32"), { node: "C:\\Program Files\\nodejs\\node.exe", npmCli: "C:\\Program Files\\nodejs\\node_modules\\npm\\bin\\npm-cli.js" });
  assert.equal(findNode({ PATH: "" }, () => false, "win32"), null);
  assert.equal(findGit(env, exists), "C:\\Program Files\\Git\\cmd\\git.exe");
  assert.equal(findMongod(env, exists, () => ["7.0", "8.0"]), "C:\\Program Files\\MongoDB\\Server\\8.0\\bin\\mongod.exe");
  assert.equal(nodeVersionOk("v20.19.0"), true);
  assert.equal(nodeVersionOk("v22.1.0"), true);
  assert.equal(nodeVersionOk("v20.18.3"), false);
  assert.equal(nodeVersionOk("v18.20.0"), false);
});

test("the host kit is built again only when its sources changed", () => {
  const ws = fakeWorkspace();
  const kit = path.join(ws, "legacy", "ReforgedHost");
  assert.equal(hostKitNeedsBuild(kit), false);
  mkdirSync(path.join(kit, "src"));
  const future = path.join(kit, "src", "cli.ts");
  writeFileSync(future, "// newer");
  const later = new Date(Date.now() + 60000);
  utimesSync(future, later, later);
  assert.equal(hostKitNeedsBuild(kit), true);
});

test("lines, errors and the page's form are cleaned and checked", () => {
  assert.equal(cleanLine("\u001b[31mred\u001b[0m\r"), "red");
  assert.equal(failureLine(["a", "Error: no MongoDB", "b"]), "no MongoDB");
  assert.equal(failureLine(["a", "last"]), "last");
  assert.deepEqual(hostSetupRequest({ name: " Friday ", mode: "private", publicHost: "x" }), { name: "Friday", mode: "private", publicHost: null });
  assert.deepEqual(hostSetupRequest({ name: "Friday", mode: "public", publicHost: "203.0.113.7" }), { name: "Friday", mode: "public", publicHost: "203.0.113.7" });
  assert.equal(hostSetupRequest({ name: "Friday", mode: "public", publicHost: "a b" }), null);
  assert.equal(hostSetupRequest({ name: "", mode: "private" }), null);
  assert.equal(hostSetupRequest({ name: "x\u0000", mode: "private" }), null);
  assert.equal(inviteFor(""), null);
  assert.equal(inviteFor("Alex"), "Alex");
  assert.equal(inviteFor("a".repeat(41)), undefined);
});

test("the test multipliers the page may ask for", () => {
  assert.equal(hostMultipliers(null), null);
  assert.deepEqual(hostMultipliers({ xp: 5, rewards: 2.505 }), { xp: 5, rewards: 2.51 });
  assert.equal(hostMultipliers({ xp: 1, rewards: 1 }), null, "1 and 1 is off");
  for (const bad of [{ xp: 0, rewards: 2 }, { xp: 101, rewards: 2 }, { xp: "5", rewards: 2 }, { xp: Number.NaN, rewards: 2 }, { xp: 5 }, [5, 5], "5"]) assert.equal(hostMultipliers(bad), undefined, JSON.stringify(bad));
});
