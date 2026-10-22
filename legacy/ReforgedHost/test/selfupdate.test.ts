import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { describe, it } from "node:test";
import { applyUpdate, checkForUpdate, runtimeAffected, UpdateError } from "../src/selfupdate";
import { tempDir, write } from "./helpers";

function git(cwd: string, ...args: string[]): string {
    return execFileSync("git", args, { cwd, encoding: "utf8", env: { ...process.env, GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.invalid", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.invalid" } }).trim();
}

function commit(repo: string, files: Record<string, string>, message: string): void {
    for (const [name, data] of Object.entries(files)) write(path.join(repo, name), data);
    git(repo, "add", "-A");
    git(repo, "commit", "--quiet", "-m", message);
}

// "GitHub" (a bare repository), the project on it, and the host's clone of it.
function project() {
    const remote = tempDir("remote");
    git(remote, "init", "--quiet", "--bare", "--initial-branch=main");
    const dev = tempDir("dev");
    git(dev, "clone", "--quiet", remote, ".");
    git(dev, "checkout", "--quiet", "-b", "main");
    commit(dev, { ".gitignore": ".workspace/\n", "README.md": "one\n", "upstream/mystic-paradox.json": JSON.stringify({ commit: "a".repeat(40), patches: [] }) }, "one");
    git(dev, "push", "--quiet", "origin", "main");
    const host = tempDir("host");
    git(host, "clone", "--quiet", remote, ".");
    write(path.join(host, ".workspace", "config", "host.json"), "{\"mine\":true}\n");
    return { remote, dev, host };
}

describe("updating the project from GitHub", () => {
    it("says when it is up to date, and fast-forwards when GitHub has more; ignored files stay", async () => {
        const { dev, host } = project();
        const before = await checkForUpdate(host);
        assert.equal(before.behind, 0);
        assert.equal(before.upstream, "origin/main");
        assert.equal((await applyUpdate(host)).updated, false);

        commit(dev, { "README.md": "two\n", "legacy/ReforgedHost/package-lock.json": "{}\n" }, "two");
        commit(dev, { "docs/x.md": "x\n" }, "three");
        git(dev, "push", "--quiet", "origin", "main");
        const check = await checkForUpdate(host);
        assert.equal(check.behind, 2);
        assert.equal(check.ahead, 0);
        const r = await applyUpdate(host);
        assert.equal(r.updated, true);
        assert.equal(r.files, 3);
        assert.equal(r.runtimeChanged, false);
        assert.equal(r.hostKitDeps, true);
        assert.equal(git(host, "show", "HEAD:README.md"), "two");
        assert.equal(git(host, "rev-parse", "HEAD"), git(dev, "rev-parse", "HEAD"));
        assert.equal(execFileSync("cat", [path.join(host, ".workspace", "config", "host.json")], { encoding: "utf8" }), "{\"mine\":true}\n");
        assert.equal((await checkForUpdate(host)).behind, 0);
    });

    it("refuses to overwrite a file the host changed, and a checkout with commits of its own", async () => {
        const { dev, host } = project();
        commit(dev, { "README.md": "two\n" }, "two");
        git(dev, "push", "--quiet", "origin", "main");
        write(path.join(host, "README.md"), "my own edit\n");
        await assert.rejects(applyUpdate(host), (e: unknown) => e instanceof UpdateError && e.code === "local_changes" && /README\.md/.test(e.detail ?? ""));
        assert.equal(execFileSync("cat", [path.join(host, "README.md")], { encoding: "utf8" }), "my own edit\n");

        git(host, "checkout", "--quiet", "--", "README.md");
        commit(host, { "mine.txt": "x\n" }, "mine");
        await assert.rejects(applyUpdate(host), (e: unknown) => e instanceof UpdateError && e.code === "diverged");
    });

    it("puts back a package-lock.json that npm install rewrote, instead of stopping", async () => {
        const { dev, host } = project();
        commit(dev, { "legacy/ReforgedHost/package-lock.json": "{\"v\":1}\n" }, "lock one");
        git(dev, "push", "--quiet", "origin", "main");
        await applyUpdate(host);
        write(path.join(host, "legacy", "ReforgedHost", "package-lock.json"), "{\"rewritten\":true}\n");
        commit(dev, { "legacy/ReforgedHost/package-lock.json": "{\"v\":2}\n" }, "lock two");
        git(dev, "push", "--quiet", "origin", "main");
        const r = await applyUpdate(host);
        assert.equal(r.updated, true);
        assert.equal(r.hostKitDeps, true);
        assert.equal(git(host, "status", "--porcelain"), "");
        assert.equal(git(host, "show", "HEAD:legacy/ReforgedHost/package-lock.json"), "{\"v\":2}");
    });

    it("needs a git checkout that follows a branch", async () => {
        await assert.rejects(checkForUpdate(tempDir("plain")), (e: unknown) => e instanceof UpdateError && e.code === "not_git");
        const { host } = project();
        git(host, "checkout", "--quiet", "--detach");
        await assert.rejects(checkForUpdate(host), (e: unknown) => e instanceof UpdateError && e.code === "no_upstream");
    });

    it("tells when the game runtime has to be built again", async () => {
        assert.equal(runtimeAffected(["upstream/patches/0006-runtime-free-leftover-worlds-before-loadmap.patch"], "a", "a"), true);
        assert.equal(runtimeAffected(["upstream/patches/0007-backend-test-xp-and-reward-multipliers.patch", "docs/hosting.md"], "a", "a"), false);
        assert.equal(runtimeAffected(["upstream/mystic-paradox.json"], "a", "b"), true);

        const { dev, host } = project();
        commit(dev, { "upstream/patches/0009-runtime-x.patch": "x\n" }, "runtime");
        git(dev, "push", "--quiet", "origin", "main");
        assert.equal((await applyUpdate(host)).runtimeChanged, true);
    });
});
