import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { ensureUpstream, readPin, upstreamState, type UpstreamPin } from "../src/upstream";
import { tempDir, write } from "./helpers";

function git(cwd: string, ...args: string[]): string {
    return execFileSync("git", args, { cwd, encoding: "utf8", env: { ...process.env, GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.invalid", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.invalid" } }).trim();
}

// A small "upstream" repository with two commits, and a patch that edits a file and adds one.
function makeOrigin() {
    const origin = tempDir("origin");
    git(origin, "init", "--quiet");
    git(origin, "config", "uploadpack.allowReachableSHA1InWant", "true");
    write(path.join(origin, ".gitignore"), "ParadoxRuntime/SDK/\nParadoxBackend/game-data/*.json\n!ParadoxBackend/game-data/*.example.json\n");
    write(path.join(origin, "ParadoxRuntime", "dllmain.cpp"), "int main() { return 0; }\n");
    write(path.join(origin, "ParadoxBackend", "game-data", "a.example.json"), "{}\n");
    git(origin, "add", "-A");
    git(origin, "commit", "--quiet", "-m", "one");
    const first = git(origin, "rev-parse", "HEAD");
    write(path.join(origin, "README.md"), "second\n");
    git(origin, "add", "-A");
    git(origin, "commit", "--quiet", "-m", "two");
    const second = git(origin, "rev-parse", "HEAD");

    // The patch, made against the first commit.
    const work = tempDir("patchwork");
    git(work, "clone", "--quiet", origin, ".");
    git(work, "checkout", "--quiet", first);
    write(path.join(work, "ParadoxRuntime", "dllmain.cpp"), "int main() { return 42; }\n");
    write(path.join(work, "ParadoxBackend", "src", "reforgedApi.ts"), "export const x = 1;\n");
    git(work, "add", "-A");
    const patch = git(work, "diff", "--cached", first);
    const patches = tempDir("patches");
    write(path.join(patches, "patches", "0001-test.patch"), patch + "\n");
    return { origin, first, second, patches };
}

describe("the pinned upstream", () => {
    it("readPin wants a full commit id and plain patch paths", () => {
        const dir = tempDir("pin");
        const file = path.join(dir, "pin.json");
        write(file, JSON.stringify({ repository: "https://github.com/pranav158/Mystic-Paradox", commit: "0".repeat(40), patches: ["patches/a.patch"] }));
        assert.equal(readPin(file).commit, "0".repeat(40));
        write(file, JSON.stringify({ repository: "https://github.com/x/y", commit: "abc", patches: [] }));
        assert.throws(() => readPin(file), /commit/);
        write(file, JSON.stringify({ repository: "https://github.com/x/y", commit: "0".repeat(40), patches: ["../../etc"] }));
        assert.throws(() => readPin(file), /patches/);
        const real = readPin(path.resolve(__dirname, "..", "..", "..", "..", "upstream", "mystic-paradox.json"));
        assert.equal(real.patches.length, 33);
    });

    it("is fetched at the pinned commit and patched; an update keeps the host's ignored files", async () => {
        const { origin, first, second, patches } = makeOrigin();
        const dir = path.join(tempDir("ws"), "Mystic-Paradox");
        const marker = path.join(dir, ".reforged-patches.json");
        const pin: UpstreamPin = { name: "test", repository: origin, commit: first, patches: ["patches/0001-test.patch"] };
        assert.equal(upstreamState(dir, marker, pin, patches), "missing");

        assert.equal(await ensureUpstream({ dir, markerFile: marker, pin, patchesRoot: patches }), "fetched");
        assert.equal(fs.readFileSync(path.join(dir, "ParadoxRuntime", "dllmain.cpp"), "utf8"), "int main() { return 42; }\n");
        assert.ok(fs.existsSync(path.join(dir, "ParadoxBackend", "src", "reforgedApi.ts")));
        assert.equal(upstreamState(dir, marker, pin, patches), "ready");
        assert.equal(await ensureUpstream({ dir, markerFile: marker, pin, patchesRoot: patches }), "already_ready");
        assert.equal(git(dir, "status", "--porcelain").split("\n").filter((l) => l.includes(".reforged-patches")).length, 0, "the marker stays out of git status");

        // The host's own files, which upstream ignores.
        write(path.join(dir, "ParadoxRuntime", "SDK", "Engine_functions.cpp"), "// sdk\n");
        write(path.join(dir, "ParadoxBackend", "game-data", "a.json"), "{\"real\":true}\n");

        const newer: UpstreamPin = { ...pin, commit: second };
        assert.equal(upstreamState(dir, marker, newer, patches), "outdated");
        assert.equal(await ensureUpstream({ dir, markerFile: marker, pin: newer, patchesRoot: patches }), "updated");
        assert.equal(fs.readFileSync(path.join(dir, "README.md"), "utf8"), "second\n");
        assert.equal(fs.readFileSync(path.join(dir, "ParadoxRuntime", "dllmain.cpp"), "utf8"), "int main() { return 42; }\n", "patched again");
        assert.equal(fs.readFileSync(path.join(dir, "ParadoxRuntime", "SDK", "Engine_functions.cpp"), "utf8"), "// sdk\n", "the SDK is kept");
        assert.equal(fs.readFileSync(path.join(dir, "ParadoxBackend", "game-data", "a.json"), "utf8"), "{\"real\":true}\n", "game data is kept");
        assert.equal(upstreamState(dir, marker, newer, patches), "ready");
    });

    it("the committed patches have LF line endings only, so reading them back to LF is exact", () => {
        const dir = path.resolve(__dirname, "..", "..", "..", "..", "upstream", "patches");
        const names = fs.readdirSync(dir).filter((n) => n.endsWith(".patch"));
        assert.equal(names.length, 33);
        for (const name of names) assert.ok(!fs.readFileSync(path.join(dir, name)).includes(0x0d), `${name} has a CR byte`);
    });

    it("applies patches checked out with CRLF line endings, as Git for Windows does by default", async () => {
        const { origin, first, patches } = makeOrigin();
        const file = path.join(patches, "patches", "0001-test.patch");
        const lf = fs.readFileSync(file, "utf8");
        fs.writeFileSync(file, lf.replace(/\n/g, "\r\n"));
        const dir = path.join(tempDir("crlf"), "Mystic-Paradox");
        const marker = path.join(dir, ".reforged-patches.json");
        const pin: UpstreamPin = { name: "test", repository: origin, commit: first, patches: ["patches/0001-test.patch"] };
        // core.autocrlf=true for the kit's git commands: upstream's files are checked out with CRLF too.
        const saved = { count: process.env.GIT_CONFIG_COUNT, key: process.env.GIT_CONFIG_KEY_0, value: process.env.GIT_CONFIG_VALUE_0 };
        Object.assign(process.env, { GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "core.autocrlf", GIT_CONFIG_VALUE_0: "true" });
        try {
            assert.equal(await ensureUpstream({ dir, markerFile: marker, pin, patchesRoot: patches }), "fetched");
        } finally {
            for (const [name, value] of [["GIT_CONFIG_COUNT", saved.count], ["GIT_CONFIG_KEY_0", saved.key], ["GIT_CONFIG_VALUE_0", saved.value]] as const) {
                if (value === undefined) delete process.env[name];
                else process.env[name] = value;
            }
        }
        assert.equal(fs.readFileSync(path.join(dir, "ParadoxRuntime", "dllmain.cpp"), "utf8"), "int main() { return 42; }\r\n");
        assert.ok(fs.existsSync(path.join(dir, "ParadoxBackend", "src", "reforgedApi.ts")));
        assert.ok(!fs.existsSync(path.join(dir, ".git", "reforged-apply.patch")), "no leftover");
        // The same patch with LF line endings is the same patch: still ready, nothing to redo.
        fs.writeFileSync(file, lf);
        assert.equal(upstreamState(dir, marker, pin, patches), "ready");
    });

    it("refuses a folder that is not a git checkout, and reports a patch that does not apply", async () => {
        const { origin, first, patches } = makeOrigin();
        const notGit = path.join(tempDir("notgit"), "Mystic-Paradox");
        write(path.join(notGit, "x.txt"), "x");
        await assert.rejects(ensureUpstream({ dir: notGit, markerFile: path.join(notGit, "m.json"), pin: { name: "t", repository: origin, commit: first, patches: [] }, patchesRoot: patches }), /not a git checkout/);
        write(path.join(patches, "patches", "bad.patch"), "--- a/nothing.txt\n+++ b/nothing.txt\n@@ -1 +1 @@\n-a\n+b\n");
        const dir = path.join(tempDir("bad"), "Mystic-Paradox");
        await assert.rejects(ensureUpstream({ dir, markerFile: path.join(dir, "m.json"), pin: { name: "t", repository: origin, commit: first, patches: ["patches/bad.patch"] }, patchesRoot: patches }), /bad\.patch does not apply/);
    });
});
