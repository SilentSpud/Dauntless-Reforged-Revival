// MongoDB for the backend. The backend runs in production mode, which needs a replica-set primary
// (its registration and economy paths use transactions); a single mongod started with --replSet
// and initiated once is enough. ReforgedHost can run that mongod itself (host.json mongo.manage),
// or the host points mongo.uri at one they run (a Windows service with `replication.replSetName`).
//
// The MongoDB driver is the backend's own (ParadoxBackend/node_modules/mongodb, installed by
// setup), so this kit has no dependencies of its own.

import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { capture } from "./run";

export const REPLICA_SET = "rs0";

// WiredTiger's cache defaults to half of (RAM - 1 GB): 7.5 GB on a 16 GB server, which the game servers
// need far more. A friends' server's database is a few megabytes, so 1 GB is plenty.
export const MONGO_CACHE_GB = 1;

export function mongodArgs(dataDir: string, logFile: string, port: number): string[] {
    return ["--replSet", REPLICA_SET, "--bind_ip", "127.0.0.1", "--port", String(port), "--dbpath", dataDir, "--logpath", logFile, "--logappend", "--wiredTigerCacheSizeGB", String(MONGO_CACHE_GB)];
}

// mongod on PATH, or in the usual Windows install folder (the newest version).
export function findMongod(env: NodeJS.ProcessEnv = process.env, platform: NodeJS.Platform = process.platform): string | null {
    const exe = platform === "win32" ? "mongod.exe" : "mongod";
    const found = capture(platform === "win32" ? "where" : "which", ["mongod"]);
    const first = found?.split(/\r?\n/)[0]?.trim();
    if (first) return first;
    if (platform === "win32") {
        const base = path.join(env.ProgramFiles ?? "C:\\Program Files", "MongoDB", "Server");
        try {
            const versions = fs.readdirSync(base).sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
            for (const v of versions) {
                const candidate = path.join(base, v, "bin", exe);
                if (fs.existsSync(candidate)) return candidate;
            }
        } catch {
            /* not installed there */
        }
    }
    return null;
}

interface MongoClientLike {
    connect(): Promise<unknown>;
    close(): Promise<void>;
    db(name: string): { command(cmd: Record<string, unknown>): Promise<Record<string, unknown>> };
}

type MongoModule = { MongoClient: new (uri: string, opts: Record<string, unknown>) => MongoClientLike };

export function loadDriver(backendDir: string): MongoModule | null {
    try {
        const req = createRequire(path.join(backendDir, "package.json"));
        return req("mongodb") as MongoModule;
    } catch {
        return null;
    }
}

// The URI without replicaSet, straight to one host: used to talk to a mongod whose replica set
// is not initiated yet (a replicaSet URI would wait for a primary that does not exist).
export function directUri(uri: string): string {
    try {
        const u = new URL(uri);
        u.searchParams.delete("replicaSet");
        u.searchParams.set("directConnection", "true");
        return u.toString();
    } catch {
        return uri;
    }
}

export type MongoCheck =
    | { ok: true; setName: string; primary: boolean; initiated: boolean }
    | { ok: false; reason: "no_driver" | "unreachable" | "standalone" | "error"; detail: string };

// Connects, and initiates the one-member replica set when the mongod was started with --replSet
// but never initiated. A standalone mongod (no --replSet) is reported, never changed.
export async function ensureReplicaSet(backendDir: string, uri: string, opts: { initiate?: boolean; host?: string; timeoutMs?: number } = {}): Promise<MongoCheck> {
    const driver = loadDriver(backendDir);
    if (!driver) return { ok: false, reason: "no_driver", detail: "the backend's packages are not installed yet (npm run setup)" };
    const client = new driver.MongoClient(directUri(uri), { serverSelectionTimeoutMS: opts.timeoutMs ?? 4000, connectTimeoutMS: opts.timeoutMs ?? 4000 });
    try {
        await client.connect();
    } catch (e) {
        return { ok: false, reason: "unreachable", detail: e instanceof Error ? e.message : String(e) };
    }
    try {
        const admin = client.db("admin");
        const hello = await admin.command({ hello: 1 });
        if (typeof hello.setName === "string" && hello.setName.length > 0) {
            return { ok: true, setName: hello.setName, primary: hello.isWritablePrimary === true, initiated: false };
        }
        // Started with --replSet but not initiated yet: "hello" answers without setName, and
        // replSetGetStatus fails with NotYetInitialized (94).
        let notYetInitialized = false;
        try {
            await admin.command({ replSetGetStatus: 1 });
        } catch (e) {
            const code = (e as { code?: number }).code;
            if (code === 94) notYetInitialized = true;
            else if (code === 76) return { ok: false, reason: "standalone", detail: "this mongod runs without a replica set; start it with --replSet rs0 (or set replication.replSetName: rs0 in mongod.cfg) and restart it" };
            else return { ok: false, reason: "error", detail: e instanceof Error ? e.message : String(e) };
        }
        if (!notYetInitialized) return { ok: false, reason: "error", detail: "unexpected replica set state" };
        if (opts.initiate === false) return { ok: false, reason: "error", detail: "the replica set is not initiated yet" };
        await admin.command({ replSetInitiate: { _id: REPLICA_SET, members: [{ _id: 0, host: opts.host ?? "127.0.0.1:27017" }] } });
        // A new one-member set elects itself within a few seconds.
        for (let i = 0; i < 40; i++) {
            const again = await admin.command({ hello: 1 });
            if (again.isWritablePrimary === true) return { ok: true, setName: String(again.setName), primary: true, initiated: true };
            await new Promise((r) => setTimeout(r, 500));
        }
        return { ok: true, setName: REPLICA_SET, primary: false, initiated: true };
    } catch (e) {
        return { ok: false, reason: "error", detail: e instanceof Error ? e.message : String(e) };
    } finally {
        await client.close().catch(() => undefined);
    }
}
