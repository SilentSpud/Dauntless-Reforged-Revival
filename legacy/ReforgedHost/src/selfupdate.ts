// Updating this project's own checkout (the folder the host cloned from GitHub) to the newest commit of
// the branch it follows. Only ever a fast-forward: git itself refuses it when it would overwrite the host's
// own changes to tracked files, and nothing git ignores (.workspace: config, secrets, game data, builds) is
// touched. Setup runs again afterwards, which re-applies the patches when they changed.

import { readPin } from "./upstream";
import { run } from "./run";

export type UpdateProblem = "not_git" | "no_upstream" | "fetch_failed" | "diverged" | "local_changes" | "merge_failed";

export class UpdateError extends Error {
    constructor(readonly code: UpdateProblem, readonly detail: string | null = null) {
        super(detail ? `${code}: ${detail}` : code);
    }
}

export interface UpdateCheck {
    branch: string; // "main"
    upstream: string; // "origin/main"
    head: string; // short commit ids
    remoteHead: string;
    behind: number; // commits on GitHub not here yet
    ahead: number; // commits here not on GitHub (the host's own)
}

export interface UpdateResult extends UpdateCheck {
    updated: boolean;
    from: string;
    to: string;
    files: number; // files changed by the update
    runtimeChanged: boolean; // the game runtime's patches or pinned commit changed: rebuild and publish it
    hostKitDeps: boolean; // ReforgedHost's dependencies changed: npm install there before it is built again
}

async function git(repo: string, args: string[], logFile?: string): Promise<{ code: number; out: string }> {
    // Never a credentials prompt: the project is public, and nobody is at the terminal to answer one.
    const r = await run("git", args, { cwd: repo, logFile, quiet: true, env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } });
    return { code: r.code, out: r.output.trim() };
}

async function gitOk(repo: string, args: string[], logFile?: string): Promise<string | null> {
    const r = await git(repo, args, logFile);
    return r.code === 0 ? r.out : null;
}

export async function checkForUpdate(repo: string, opts: { logFile?: string; fetch?: boolean } = {}): Promise<UpdateCheck> {
    if ((await gitOk(repo, ["rev-parse", "--is-inside-work-tree"])) !== "true") throw new UpdateError("not_git");
    const branch = (await gitOk(repo, ["rev-parse", "--abbrev-ref", "HEAD"])) ?? "HEAD";
    const upstream = await gitOk(repo, ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);
    if (!upstream || branch === "HEAD") throw new UpdateError("no_upstream", branch);
    if (opts.fetch !== false) {
        const remote = upstream.split("/")[0];
        const r = await git(repo, ["fetch", "--quiet", remote], opts.logFile);
        if (r.code !== 0) throw new UpdateError("fetch_failed", r.out.split(/\r?\n/).filter(Boolean).pop() ?? null);
    }
    const counts = (await gitOk(repo, ["rev-list", "--left-right", "--count", `HEAD...${upstream}`])) ?? "0 0";
    const [ahead, behind] = counts.split(/\s+/).map((n) => Number(n) || 0);
    return {
        branch,
        upstream,
        head: (await gitOk(repo, ["rev-parse", "--short=12", "HEAD"])) ?? "",
        remoteHead: (await gitOk(repo, ["rev-parse", "--short=12", upstream])) ?? "",
        behind,
        ahead,
    };
}

// The runtime the players load is built from the patched Mystic Paradox: a changed runtime patch, or a new
// pinned commit, means the host rebuilds MysticParadox.dll and publishes it again.
export function runtimeAffected(files: string[], pinBefore: string | null, pinAfter: string | null): boolean {
    if (files.some((f) => /^upstream\/patches\/\d+-runtime-/.test(f))) return true;
    return pinBefore !== null && pinAfter !== null && pinBefore !== pinAfter;
}

function pinCommit(json: string | null): string | null {
    if (json === null) return null;
    try {
        const commit = (JSON.parse(json) as { commit?: unknown }).commit;
        return typeof commit === "string" ? commit : null;
    } catch {
        return null;
    }
}

// npm install rewrites a package-lock.json with another npm version: those are put back as committed before
// an update (npm install makes them again), so they never stop it. Returns the files put back.
export async function restoreLockfiles(repo: string, logFile?: string): Promise<string[]> {
    const changed = (await gitOk(repo, ["diff", "--name-only", "HEAD"])) ?? "";
    const locks = changed.split(/\r?\n/).map((l) => l.trim()).filter((f) => /(^|\/)package-lock\.json$/.test(f));
    if (locks.length) await git(repo, ["checkout", "--", ...locks], logFile);
    return locks;
}

export async function applyUpdate(repo: string, opts: { logFile?: string } = {}): Promise<UpdateResult> {
    const check = await checkForUpdate(repo, opts);
    const none: UpdateResult = { ...check, updated: false, from: check.head, to: check.head, files: 0, runtimeChanged: false, hostKitDeps: false };
    if (check.behind === 0) return none;
    if (check.ahead > 0) throw new UpdateError("diverged", `${check.ahead} local commit(s)`);
    const from = (await gitOk(repo, ["rev-parse", "HEAD"]))!;
    const pinBefore = pinCommit(await gitOk(repo, ["show", `${from}:upstream/mystic-paradox.json`]));
    const restored = await restoreLockfiles(repo, opts.logFile);
    const merge = await git(repo, ["merge", "--ff-only", "--quiet", check.upstream], opts.logFile);
    if (merge.code !== 0) {
        // "Your local changes to the following files would be overwritten by merge:" then the files, indented.
        const files = merge.out.split(/\r?\n/).filter((l) => /^\s+\S/.test(l)).map((l) => l.trim());
        if (/would be overwritten|untracked working tree files/i.test(merge.out)) throw new UpdateError("local_changes", files.slice(0, 5).join(", ") || null);
        throw new UpdateError("merge_failed", merge.out.split(/\r?\n/).filter(Boolean).pop() ?? null);
    }
    const to = (await gitOk(repo, ["rev-parse", "HEAD"]))!;
    const changed = ((await gitOk(repo, ["diff", "--name-only", from, to])) ?? "").split(/\r?\n/).filter(Boolean);
    let pinAfter: string | null = null;
    try {
        pinAfter = readPin(`${repo}/upstream/mystic-paradox.json`).commit;
    } catch {
        pinAfter = null;
    }
    return {
        ...check,
        updated: true,
        from: from.slice(0, 12),
        to: to.slice(0, 12),
        head: to.slice(0, 12),
        behind: 0,
        files: changed.length,
        runtimeChanged: runtimeAffected(changed, pinBefore, pinAfter),
        hostKitDeps: restored.includes("legacy/ReforgedHost/package-lock.json") || changed.some((f) => f === "legacy/ReforgedHost/package.json" || f === "legacy/ReforgedHost/package-lock.json"),
    };
}
