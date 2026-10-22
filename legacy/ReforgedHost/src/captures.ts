// The live store captures Mystic Paradox's store generators read: Lady Luck's rotation
// (scripts/enrich_ladyluck_store.ts) and the platinum store's prices, Hunt Pass rank skips and passes
// (scripts/generate_platinum_store.ts), both from <upstream>/DauntlessEndpointDocumentation/Store/Product/Skus/.
// Without them Lady Luck's Store is the shipped example and the platinum store has no rank skips, no
// passes and no captured prices.
//
// They come from EisigesEis/DauntlessEndpointDocumentation, which carries no license, so they are never
// copied into this repository: upstream/store-captures.json pins the commit and each file's SHA-256, and
// `npm run gamedata` fetches that commit on the host's own machine (a shallow fetch into
// .workspace/captures/) and writes the files where the generators look. A file whose bytes do not match its
// pinned hash is refused.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { run } from "./run";
import type { Paths } from "./paths";

export interface CapturePin {
    name: string;
    repository: string;
    commit: string;
    files: Record<string, string>; // path in the repository -> SHA-256
}

export function readCapturePin(file: string): CapturePin {
    const raw = JSON.parse(fs.readFileSync(file, "utf8")) as Partial<CapturePin>;
    if (typeof raw.repository !== "string" || !/^(https:\/\/|file:\/\/|\/|[A-Za-z]:\\)/.test(raw.repository)) throw new Error(`${file}: repository must be an https URL`);
    if (typeof raw.commit !== "string" || !/^[0-9a-f]{40}$/.test(raw.commit)) throw new Error(`${file}: commit must be a full 40-character commit id`);
    const files = raw.files ?? {};
    for (const [name, hash] of Object.entries(files)) {
        if (name.includes("..") || path.isAbsolute(name) || !/^[0-9a-f]{64}$/.test(hash)) throw new Error(`${file}: files must map plain paths to SHA-256 hashes`);
    }
    if (Object.keys(files).length === 0) throw new Error(`${file}: no files`);
    return { name: raw.name ?? "DauntlessEndpointDocumentation", repository: raw.repository, commit: raw.commit, files };
}

const sha256 = (data: Buffer) => createHash("sha256").update(data).digest("hex");

function target(p: Paths, name: string): string {
    return path.join(p.storeCaptures, ...name.split("/"));
}

// The files that are not in place with their pinned bytes yet.
export function missingCaptures(p: Paths, pin: CapturePin): string[] {
    return Object.entries(pin.files)
        .filter(([name, hash]) => {
            try {
                return sha256(fs.readFileSync(target(p, name))) !== hash;
            } catch {
                return true;
            }
        })
        .map(([name]) => name);
}

async function git(args: string[], cwd: string, logFile?: string): Promise<void> {
    const r = await run("git", args, { cwd, logFile, quiet: true });
    if (r.code !== 0) throw new Error(`git ${args[0]} failed: ${r.lastLine ?? `exit ${r.code}`}`);
}

// Puts the pinned files in place, fetching the commit when one is missing or changed. The files are read
// out of git's objects (`git show <commit>:<path>`), so the bytes are the committed ones whatever the
// host's line-ending settings are.
export async function ensureCaptures(p: Paths, pin: CapturePin, logFile?: string): Promise<"already_ready" | "fetched"> {
    if (missingCaptures(p, pin).length === 0) return "already_ready";
    const repo = path.join(p.workspace, "captures", `${pin.name}.git`);
    if (!fs.existsSync(path.join(repo, "HEAD"))) {
        fs.mkdirSync(repo, { recursive: true });
        await git(["init", "--quiet", "--bare"], repo, logFile);
    }
    await git(["-c", "protocol.version=2", "fetch", "--quiet", "--depth", "1", pin.repository, pin.commit], repo, logFile);
    const data = new Map<string, Buffer>();
    for (const [name, hash] of Object.entries(pin.files)) {
        let bytes: Buffer;
        try {
            bytes = execFileSync("git", ["show", `${pin.commit}:${name}`], { cwd: repo, maxBuffer: 64 * 1024 * 1024, windowsHide: true, stdio: ["ignore", "pipe", "ignore"] });
        } catch {
            throw new Error(`${pin.name} has no ${name} at ${pin.commit.slice(0, 12)}`);
        }
        if (sha256(bytes) !== hash) throw new Error(`${pin.name}'s ${name} does not match its pinned SHA-256`);
        data.set(name, bytes);
    }
    for (const [name, bytes] of data) {
        fs.mkdirSync(path.dirname(target(p, name)), { recursive: true });
        fs.writeFileSync(target(p, name), bytes);
    }
    // Not part of upstream: keep the folder out of its `git status`.
    const exclude = path.join(p.upstream, ".git", "info", "exclude");
    if (fs.existsSync(path.join(p.upstream, ".git"))) {
        const entry = `/${path.basename(p.storeCaptures)}/`;
        const current = fs.existsSync(exclude) ? fs.readFileSync(exclude, "utf8") : "";
        if (!current.split(/\r?\n/).includes(entry)) {
            fs.mkdirSync(path.dirname(exclude), { recursive: true });
            fs.appendFileSync(exclude, `\n# Dauntless Reforged Revival: the store captures (legacy/ReforgedHost/src/captures.ts)\n${entry}\n`);
        }
    }
    return "fetched";
}
