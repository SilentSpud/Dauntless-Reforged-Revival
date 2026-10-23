// Game data packs: everything `npm run gamedata` made, in one file, so a host who has generated the game
// data once can set up the next server (or the same one again) without the SDK, CatalogExporter and the
// generators. `npm run gamedata -- --export` writes one; `npm run gamedata -- --import <file>` (or the
// launcher's Host page) puts it back.
//
// A pack is gzip-compressed JSON: { format, version, game, created, files: [{ path, sha256, data }] }.
// Only the known game-data files go in and come out (the backend's and the Director's, and the sources
// in the backend's game-data/raw), so a pack can never write anywhere else. Game data is Phoenix Labs'
// data generated from the host's own game: a pack is for the host's own use, not for publishing.

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { BACKEND_GAME_DATA, DIRECTOR_GAME_DATA, SUPPLIED_SOURCES } from "./gamedata";
import { GAME_CHANGELIST, type Paths } from "./paths";

export const PACK_FORMAT = "dauntless-reforged-revival-gamedata";
export const PACK_VERSION = 1;
export const PACK_GAME = `1.14.7 (${GAME_CHANGELIST})`;

interface PackFile {
    path: string;
    sha256: string;
    data: string;
}

interface Pack {
    format: string;
    version: number;
    game: string;
    created: string;
    files: PackFile[];
}

// Pack path -> file on disk, for every file a pack may hold.
export function packEntries(p: Paths): Map<string, string> {
    const entries = new Map<string, string>();
    for (const name of BACKEND_GAME_DATA) entries.set(`backend/${name}`, path.join(p.backendGameData, name));
    for (const name of DIRECTOR_GAME_DATA) entries.set(`director/${name}`, path.join(p.directorGameData, name));
    for (const name of Object.values(SUPPLIED_SOURCES)) entries.set(`backend/raw/${name}`, path.join(p.rawGameData, name));
    return entries;
}

const sha256 = (data: string) => createHash("sha256").update(data, "utf8").digest("hex");

function isExample(file: string, data: string): boolean {
    const example = file.replace(/\.json$/, ".example.json");
    return fs.existsSync(example) && fs.readFileSync(example, "utf8") === data;
}

export function defaultPackFile(p: Paths, now = new Date()): string {
    const stamp = now.toISOString().slice(0, 16).replace(/[-:]/g, "").replace("T", "-");
    return path.join(p.workspace, "gamedata-packs", `reforged-gamedata-1.14.7-${stamp}.json.gz`);
}

// Writes the pack; the shipped examples are left out (a pack holds generated data only).
export function exportGameDataPack(p: Paths, outFile: string, now = new Date()): { file: string; included: string[]; left: string[] } {
    const files: PackFile[] = [];
    const left: string[] = [];
    for (const [packPath, file] of packEntries(p)) {
        if (!fs.existsSync(file)) {
            left.push(packPath);
            continue;
        }
        const data = fs.readFileSync(file, "utf8");
        if (isExample(file, data)) {
            left.push(packPath);
            continue;
        }
        files.push({ path: packPath, sha256: sha256(data), data });
    }
    if (files.length === 0) throw new Error("no generated game data to pack yet (npm run gamedata first)");
    const pack: Pack = { format: PACK_FORMAT, version: PACK_VERSION, game: PACK_GAME, created: now.toISOString(), files };
    fs.mkdirSync(path.dirname(outFile), { recursive: true });
    fs.writeFileSync(outFile, zlib.gzipSync(Buffer.from(JSON.stringify(pack), "utf8")));
    return { file: outFile, included: files.map((f) => f.path), left };
}

// Reads and checks a pack: its format, its game build, and each file's name and hash.
export function readGameDataPack(p: Paths, file: string): Pack {
    let pack: Pack;
    try {
        pack = JSON.parse(zlib.gunzipSync(fs.readFileSync(file)).toString("utf8")) as Pack;
    } catch {
        throw new Error(`${file} is not a game data pack (a .json.gz file from npm run gamedata -- --export)`);
    }
    if (pack?.format !== PACK_FORMAT || pack.version !== PACK_VERSION || !Array.isArray(pack.files)) throw new Error(`${file} is not a game data pack of this version`);
    if (pack.game !== PACK_GAME) throw new Error(`${file} holds game data for Dauntless ${pack.game}; this server runs ${PACK_GAME}`);
    const entries = packEntries(p);
    for (const f of pack.files) {
        if (typeof f?.path !== "string" || typeof f.data !== "string" || !entries.has(f.path)) throw new Error(`${file} holds an unknown file: ${String(f?.path)}`);
        if (sha256(f.data) !== f.sha256) throw new Error(`${file} is damaged: ${f.path} does not match its hash`);
        try {
            JSON.parse(f.data);
        } catch {
            throw new Error(`${file} is damaged: ${f.path} is not JSON`);
        }
    }
    return pack;
}

// Puts a pack's files in place. A file that is there and different is kept beside it as <name>.<time>.bak.
export function importGameDataPack(p: Paths, file: string, now = new Date()): { written: string[]; unchanged: string[]; backups: string[] } {
    const pack = readGameDataPack(p, file);
    const entries = packEntries(p);
    const stamp = now.toISOString().replace(/[:.]/g, "-");
    const out = { written: [] as string[], unchanged: [] as string[], backups: [] as string[] };
    for (const f of pack.files) {
        const target = entries.get(f.path)!;
        if (fs.existsSync(target)) {
            const current = fs.readFileSync(target, "utf8");
            if (current === f.data) {
                out.unchanged.push(f.path);
                continue;
            }
            const backup = `${target}.${stamp}.bak`;
            fs.renameSync(target, backup);
            out.backups.push(backup);
        }
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.writeFileSync(target, f.data);
        out.written.push(f.path);
    }
    return out;
}
