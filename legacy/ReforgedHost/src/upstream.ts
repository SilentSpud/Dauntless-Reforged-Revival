// The pinned Mystic Paradox source in .workspace/Mystic-Paradox, with the Reforged patches applied.
//
// It is a git checkout of exactly one commit (shallow fetch by commit id). The patches are applied
// with `git apply --index`, so they are staged: updating later is `git reset --hard <new commit>`,
// which puts every tracked file back to upstream and removes the files the patches had added, while
// leaving alone everything git ignores there: the host's SDK, game data, exports and builds. Then
// the patches go on again. Nothing of the host's own is ever deleted.

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { capture, run } from "./run";

export interface UpstreamPin {
    name: string;
    repository: string;
    commit: string;
    patches: string[];
}

export interface PatchMarker {
    repository: string;
    commit: string;
    patches: { name: string; sha256: string }[];
    appliedAt: string;
}

export function readPin(file: string): UpstreamPin {
    const raw = JSON.parse(fs.readFileSync(file, "utf8")) as Partial<UpstreamPin>;
    if (typeof raw.repository !== "string" || !/^(https:\/\/|file:\/\/|\/|[A-Za-z]:\\)/.test(raw.repository)) throw new Error(`${file}: repository must be an https URL`);
    if (typeof raw.commit !== "string" || !/^[0-9a-f]{40}$/.test(raw.commit)) throw new Error(`${file}: commit must be a full 40-character commit id`);
    if (!Array.isArray(raw.patches) || raw.patches.some((p) => typeof p !== "string" || p.includes(".."))) throw new Error(`${file}: patches must be a list of paths`);
    return { name: raw.name ?? "Mystic Paradox", repository: raw.repository, commit: raw.commit, patches: raw.patches as string[] };
}

// A patch as git must read it. Git for Windows checks text files out with CRLF line endings by
// default (core.autocrlf=true), and a patch whose lines end in CRLF does not match the files it
// changes. The patches are committed with LF line endings only (a test checks that), so turning
// CRLF back into LF gives exactly the committed patch, however the repository was checked out.
export function patchBytes(file: string): Buffer {
    return Buffer.from(fs.readFileSync(file).toString("latin1").replace(/\r\n/g, "\n"), "latin1");
}

function sha256(data: Buffer): string {
    return createHash("sha256").update(data).digest("hex");
}

export function expectedMarker(pin: UpstreamPin, patchesRoot: string): Omit<PatchMarker, "appliedAt"> {
    return {
        repository: pin.repository,
        commit: pin.commit,
        patches: pin.patches.map((p) => ({ name: path.basename(p), sha256: sha256(patchBytes(path.join(patchesRoot, p))) })),
    };
}

export function readMarker(file: string): PatchMarker | null {
    try {
        return JSON.parse(fs.readFileSync(file, "utf8")) as PatchMarker;
    } catch {
        return null;
    }
}

export type UpstreamState = "missing" | "not_git" | "unpatched" | "outdated" | "ready";

// "outdated": patched, but for another commit or other patches than the repository names now.
export function upstreamState(dir: string, markerFile: string, pin: UpstreamPin, patchesRoot: string): UpstreamState {
    if (!fs.existsSync(dir)) return "missing";
    if (!fs.existsSync(path.join(dir, ".git"))) return "not_git";
    const marker = readMarker(markerFile);
    if (marker === null) return "unpatched";
    const want = expectedMarker(pin, patchesRoot);
    const same = marker.commit === want.commit && JSON.stringify(marker.patches) === JSON.stringify(want.patches);
    return same ? "ready" : "outdated";
}

export function gitAvailable(): boolean {
    return capture("git", ["--version"]) !== null;
}

async function git(args: string[], cwd: string, logFile?: string): Promise<void> {
    const r = await run("git", args, { cwd, logFile, quiet: true });
    if (r.code !== 0) throw new Error(`git ${args[0]} failed: ${r.lastLine ?? `exit ${r.code}`}`);
}

// Fetches the pinned commit (if needed) and applies the patches. Returns what it did.
export async function ensureUpstream(opts: { dir: string; markerFile: string; pin: UpstreamPin; patchesRoot: string; logFile?: string; force?: boolean }): Promise<"already_ready" | "fetched" | "updated"> {
    const { dir, pin } = opts;
    const state = upstreamState(dir, opts.markerFile, pin, opts.patchesRoot);
    if (state === "ready" && !opts.force) return "already_ready";
    if (state === "not_git") {
        throw new Error(`${dir} exists but is not a git checkout. Move it away (keep its SDK and game-data files) and run setup again.`);
    }
    const fresh = state === "missing";
    if (fresh) {
        fs.mkdirSync(dir, { recursive: true });
        await git(["init", "--quiet"], dir, opts.logFile);
        await git(["remote", "add", "origin", pin.repository], dir, opts.logFile);
    } else {
        await git(["remote", "set-url", "origin", pin.repository], dir, opts.logFile);
    }
    // Only this commit, without history. A commit id fetch works on GitHub and for local paths.
    await git(["-c", "protocol.version=2", "fetch", "--quiet", "--depth", "1", "origin", pin.commit], dir, opts.logFile);
    // Back to upstream exactly: tracked files reset, the files a patch had added removed; ignored
    // files (the host's SDK, game data, builds) stay.
    await git(["-c", "advice.detachedHead=false", "checkout", "--quiet", "--force", "--detach", pin.commit], dir, opts.logFile);
    await git(["reset", "--quiet", "--hard", pin.commit], dir, opts.logFile);
    try {
        fs.rmSync(opts.markerFile, { force: true });
    } catch {
        /* none */
    }
    const applying = path.join(dir, ".git", "reforged-apply.patch");
    try {
        for (const patch of pin.patches) {
            const file = path.join(opts.patchesRoot, patch);
            fs.writeFileSync(applying, patchBytes(file));
            await git(["apply", "--index", "--whitespace=nowarn", applying], dir, opts.logFile).catch((e: Error) => {
                throw new Error(`${path.basename(file)} does not apply to ${pin.commit.slice(0, 12)}: ${e.message}`);
            });
        }
    } finally {
        fs.rmSync(applying, { force: true });
    }
    const marker: PatchMarker = { ...expectedMarker(pin, opts.patchesRoot), appliedAt: new Date().toISOString() };
    fs.writeFileSync(opts.markerFile, JSON.stringify(marker, null, 2) + "\n");
    // The marker itself is never part of upstream: keep it out of `git status`.
    const exclude = path.join(dir, ".git", "info", "exclude");
    const current = fs.existsSync(exclude) ? fs.readFileSync(exclude, "utf8") : "";
    if (!current.includes(".reforged-patches.json")) {
        fs.mkdirSync(path.dirname(exclude), { recursive: true });
        fs.appendFileSync(exclude, "\n# Dauntless Reforged Revival\n.reforged-patches.json\n");
    }
    return fresh ? "fetched" : "updated";
}
