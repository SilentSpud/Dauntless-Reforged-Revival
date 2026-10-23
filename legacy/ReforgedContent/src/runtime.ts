import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { logger } from "./log";

// The game runtime this server hands to players: MysticParadox.dll and winmm.dll, built by the
// host from their own SDK and copied into CONTENT_RUNTIME_DIR with a runtime.json next to them
// (ReforgedHost's `npm run publish-runtime`, or the launcher's Host page):
//
//   { "version": "20261014-1a2b3c4d", "build": { "changelist": 647472 },
//     "files": [ { "name": "MysticParadox.dll", "size": 4718592, "sha256": "<64 hex>" },
//                { "name": "winmm.dll", "size": 233472, "sha256": "<64 hex>" } ] }
//
// The list is served only after every file on disk matched its size and SHA-256, so a player is
// never offered a list the server cannot keep. The folder is checked again every RECHECK_MS, and a
// new runtime.json (or a file that changed under it) is picked up without a restart. The launcher
// checks every byte against the list again, and asks the player before installing it.

export const RUNTIME_FILE_NAMES = ["MysticParadox.dll", "winmm.dll"] as const;
export const MAX_RUNTIME_FILE_BYTES = 64 * 1024 * 1024;
const MAX_INDEX_BYTES = 64 * 1024;
const RECHECK_MS = 10 * 1000;

export type RuntimeFile = { name: string; size: number; sha256: string };

export type RuntimeIndex = {
    version: string;
    build: { name: string; changelist: number };
    files: RuntimeFile[];
};

export type RuntimeStatus =
    | { status: "ok"; index: RuntimeIndex; body: string; etag: string }
    | { status: "none"; reason: string }
    | { status: "invalid"; reason: string };

// Throws with the reason when runtime.json is not exactly the expected shape.
export function ParseRuntimeIndex(Raw: unknown): RuntimeIndex {
    if(Raw === null || typeof Raw !== "object" || Array.isArray(Raw)){
        throw new Error("runtime.json must be an object");
    }
    const Obj = Raw as Record<string, unknown>;
    const Version = typeof Obj.version === "string" ? Obj.version.trim() : "";
    if(!/^[A-Za-z0-9._+-]{1,48}$/.test(Version)){
        throw new Error("version must be 1-48 letters, digits, dots, dashes, plus signs or underscores");
    }
    const Build = Obj.build !== null && typeof Obj.build === "object" ? Obj.build as Record<string, unknown> : {};
    const Changelist = Build.changelist === undefined ? 647472 : Build.changelist;
    if(typeof Changelist !== "number" || !Number.isSafeInteger(Changelist) || Changelist <= 0){
        throw new Error("build.changelist must be a positive whole number");
    }
    if(!Array.isArray(Obj.files)){
        throw new Error("files must be a list");
    }
    const Files: RuntimeFile[] = [];
    for(const Entry of Obj.files as unknown[]){
        const F = Entry !== null && typeof Entry === "object" ? Entry as Record<string, unknown> : {};
        if(typeof F.name !== "string" || !(RUNTIME_FILE_NAMES as readonly string[]).includes(F.name)){
            throw new Error(`unknown file ${JSON.stringify(F.name)} (only ${RUNTIME_FILE_NAMES.join(" and ")})`);
        }
        if(Files.some((Other) => Other.name === F.name)){
            throw new Error(`${F.name} is listed twice`);
        }
        if(typeof F.size !== "number" || !Number.isSafeInteger(F.size) || F.size < 1 || F.size > MAX_RUNTIME_FILE_BYTES){
            throw new Error(`${F.name}: size must be 1 to ${MAX_RUNTIME_FILE_BYTES} bytes`);
        }
        if(typeof F.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(F.sha256)){
            throw new Error(`${F.name}: sha256 must be 64 lowercase hex characters`);
        }
        Files.push({ name: F.name, size: F.size, sha256: F.sha256 });
    }
    for(const Name of RUNTIME_FILE_NAMES){
        if(!Files.some((F) => F.name === Name)){
            throw new Error(`${Name} is missing from the list`);
        }
    }
    Files.sort((A, B) => (A.name < B.name ? -1 : 1));
    return { version: Version, build: { name: "rel-1.14.7-Archon", changelist: Changelist }, files: Files };
}

function HashFileSync(File: string): string {
    return createHash("sha256").update(fs.readFileSync(File)).digest("hex");
}

export class RuntimeStore {
    private Current: RuntimeStatus;
    private Signature = "";
    private CheckedAt = -Infinity;

    constructor(private readonly Dir: string | undefined, private readonly Now: () => number = Date.now){
        this.Current = Dir === undefined ? { status: "none", reason: "CONTENT_RUNTIME_DIR is not set" } : { status: "none", reason: "not checked yet" };
    }

    get(): RuntimeStatus {
        this.Refresh();
        return this.Current;
    }

    // The path of a listed file, or undefined. The name must be exactly one in the checked list.
    file(Name: string): { file: RuntimeFile; fullPath: string } | undefined {
        const Status = this.get();
        if(Status.status !== "ok" || this.Dir === undefined){
            return undefined;
        }
        const Found = Status.index.files.find((F) => F.name === Name);
        return Found === undefined ? undefined : { file: Found, fullPath: path.join(this.Dir, Found.name) };
    }

    // Forget the last check (a file failed while being served).
    invalidate(){
        this.CheckedAt = -Infinity;
        this.Signature = "";
    }

    private SignatureNow(): string {
        const Dir = this.Dir as string;
        const Parts: string[] = [];
        for(const Name of ["runtime.json", ...RUNTIME_FILE_NAMES]){
            try{
                const Stat = fs.statSync(path.join(Dir, Name));
                Parts.push(`${Name}:${Stat.size}:${Stat.mtimeMs}`);
            }
            catch{
                Parts.push(`${Name}:missing`);
            }
        }
        return Parts.join("|");
    }

    private Refresh(){
        if(this.Dir === undefined || this.Now() - this.CheckedAt < RECHECK_MS){
            return;
        }
        this.CheckedAt = this.Now();
        const Signature = this.SignatureNow();
        if(Signature === this.Signature){
            return;
        }
        this.Signature = Signature;
        const Previous = this.Current;
        this.Current = this.Load();
        if(this.Current.status === "ok"){
            if(Previous.status !== "ok" || Previous.index.version !== this.Current.index.version){
                logger.info("runtime ready", { version: this.Current.index.version, files: this.Current.index.files });
            }
        }
        else if(Previous.status === "ok" || Previous.reason !== this.Current.reason){
            (this.Current.status === "invalid" ? logger.warn : logger.info)("no runtime is offered to players", { dir: this.Dir, reason: this.Current.reason });
        }
    }

    private Load(): RuntimeStatus {
        const Dir = this.Dir as string;
        const IndexFile = path.join(Dir, "runtime.json");
        let Text: string;
        try{
            const Stat = fs.statSync(IndexFile);
            if(!Stat.isFile() || Stat.size > MAX_INDEX_BYTES){
                return { status: "invalid", reason: "runtime.json is not a regular file under 64 KB" };
            }
            Text = fs.readFileSync(IndexFile, "utf8").replace(/^﻿/, "");
        }
        catch{
            return { status: "none", reason: "no runtime.json yet (the host has not published a runtime)" };
        }
        let Index: RuntimeIndex;
        try{
            Index = ParseRuntimeIndex(JSON.parse(Text));
        }
        catch(error){
            return { status: "invalid", reason: `runtime.json: ${error instanceof Error ? error.message : String(error)}` };
        }
        for(const F of Index.files){
            const Full = path.join(Dir, F.name);
            try{
                const Stat = fs.lstatSync(Full);
                if(!Stat.isFile()){
                    return { status: "invalid", reason: `${F.name} is not a regular file` };
                }
                if(Stat.size !== F.size){
                    return { status: "invalid", reason: `${F.name} is ${Stat.size} bytes, runtime.json says ${F.size}` };
                }
                if(HashFileSync(Full) !== F.sha256){
                    return { status: "invalid", reason: `${F.name} does not match its sha256 in runtime.json` };
                }
            }
            catch{
                return { status: "invalid", reason: `${F.name} is missing` };
            }
        }
        const Body = JSON.stringify(Index);
        return { status: "ok", index: Index, body: Body, etag: `"${createHash("sha256").update(Body).digest("hex")}"` };
    }
}
