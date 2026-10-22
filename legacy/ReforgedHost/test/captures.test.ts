import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { ensureCaptures, missingCaptures, readCapturePin, type CapturePin } from "../src/captures";
import { gameDataSteps, generateGameData } from "../src/gamedata";
import { paths } from "../src/paths";
import { tempDir, write } from "./helpers";

function git(cwd: string, ...args: string[]): string {
    return execFileSync("git", args, { cwd, encoding: "utf8", env: { ...process.env, GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.invalid", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.invalid" } }).trim();
}

const sha256 = (data: string) => createHash("sha256").update(data).digest("hex");

// A capture repository with one store file, committed with CRLF line endings to check the bytes are
// the committed ones.
function makeCaptures(): { pin: CapturePin; data: string } {
    const origin = tempDir("captures-origin");
    git(origin, "init", "--quiet");
    git(origin, "config", "uploadpack.allowReachableSHA1InWant", "true");
    git(origin, "config", "core.autocrlf", "false");
    const data = "[\r\n  {\"id\": \"trials_pr_frank\"}\r\n]\r\n";
    write(path.join(origin, "Store", "Product", "Skus", "ladyluckstore.json"), data);
    git(origin, "add", "-A");
    git(origin, "commit", "--quiet", "-m", "captures");
    return { pin: { name: "Captures", repository: origin, commit: git(origin, "rev-parse", "HEAD"), files: { "Store/Product/Skus/ladyluckstore.json": sha256(data) } }, data };
}

describe("the store captures", () => {
    it("the committed pin names a commit and a hash for every store file the generators read", () => {
        const pin = readCapturePin(path.resolve(__dirname, "..", "..", "..", "..", "upstream", "store-captures.json"));
        assert.equal(pin.repository, "https://github.com/EisigesEis/DauntlessEndpointDocumentation");
        assert.deepEqual(Object.keys(pin.files).map((f) => path.posix.basename(f)).sort(), ["awakening_webstore.json", "dyes.json", "hp_level_skip_rank.json", "huntpass_store.json", "ladyluckstore.json"]);
        const file = path.join(tempDir("capture-pin"), "pin.json");
        write(file, JSON.stringify({ repository: "https://x/y", commit: "0".repeat(40), files: { "../evil.json": "0".repeat(64) } }));
        assert.throws(() => readCapturePin(file), /plain paths/);
    });

    it("are fetched at the pinned commit into the folder the generators read, and checked", async () => {
        const { pin, data } = makeCaptures();
        const p = paths(tempDir("captures-host"));
        fs.mkdirSync(path.join(p.upstream, ".git", "info"), { recursive: true });
        assert.deepEqual(missingCaptures(p, pin), ["Store/Product/Skus/ladyluckstore.json"]);
        assert.equal(await ensureCaptures(p, pin), "fetched");
        const file = path.join(p.upstream, "DauntlessEndpointDocumentation", "Store", "Product", "Skus", "ladyluckstore.json");
        assert.equal(fs.readFileSync(file, "utf8"), data, "byte for byte");
        assert.deepEqual(missingCaptures(p, pin), []);
        assert.equal(await ensureCaptures(p, pin), "already_ready");
        assert.match(fs.readFileSync(path.join(p.upstream, ".git", "info", "exclude"), "utf8"), /^\/DauntlessEndpointDocumentation\/$/m);

        // A file that does not match its pinned hash is never written.
        const q = paths(tempDir("captures-bad"));
        await assert.rejects(ensureCaptures(q, { ...pin, files: { "Store/Product/Skus/ladyluckstore.json": "0".repeat(64) } }), /does not match/);
        await assert.rejects(ensureCaptures(q, { ...pin, files: { "Store/Product/Skus/nothing.json": "0".repeat(64) } }), /has no Store\/Product\/Skus\/nothing\.json/);
        assert.ok(!fs.existsSync(q.storeCaptures));
    });

    it("build Lady Luck's Store from the live rotation unless the host supplies a response", async () => {
        const { pin } = makeCaptures();
        const p = paths(tempDir("captures-steps"));
        write(p.capturesPin, JSON.stringify(pin));
        write(path.join(p.exports, "catalog_1_14_7.jsonl"), "{}\n");
        write(path.join(p.backendGameData, "ladyluck_store.example.json"), "[]\n");
        const ladyLuck = () => gameDataSteps(p, { examplesForMissing: false }).find((s) => s.title === "Lady Luck store")!;
        assert.match(ladyLuck().skipIf?.() ?? "", /store captures are not there/);
        await ensureCaptures(p, pin);
        assert.deepEqual(ladyLuck().args, ["run", "ladyluck:enrich"]);
        assert.equal(ladyLuck().skipIf?.(), null);

        // The enrichment runs on an existing store file: the example goes in first.
        write(path.join(p.backendDir, "package.json"), JSON.stringify({ name: "b", private: true, scripts: { "ladyluck:enrich": "node -e \"require('fs').statSync('game-data/ladyluck_store.json')\"" } }));
        const lines: string[] = [];
        const fetched: string[] = [];
        const r = await generateGameData(p, { examplesForMissing: false, logFile: path.join(p.logs, "gamedata.log"), print: (l) => lines.push(l), only: ["Lady Luck store"], fetchCaptures: async (_p, q) => (fetched.push(q.commit), "already_ready") });
        assert.deepEqual(r, { ok: true, failed: [] }, lines.join("\n"));
        assert.deepEqual(fetched, [pin.commit]);
        assert.ok(lines.some((l) => l.startsWith("- Store captures (Captures at ")), lines.join("\n"));

        write(path.join(p.rawGameData, "ladyluck_store.source.json"), "[]\n");
        assert.deepEqual(ladyLuck().args, ["run", "generate:ladyluck-store", "--", "--apply"], "a supplied response wins");
    });
});
