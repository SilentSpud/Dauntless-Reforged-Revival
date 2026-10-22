import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import http from "node:http";

export type Reply = {
    status: number;
    headers: http.IncomingHttpHeaders;
    body: Buffer;
};

// A raw request: the path goes on the wire exactly as given (no dot-segment clean-up, no encoding),
// which is what the traversal tests need.
export function Request(Port: number, Method: string, RawPath: string, Headers: Record<string, string> = {}): Promise<Reply> {
    return new Promise((resolve, reject) => {
        const Req = http.request({ host: "127.0.0.1", port: Port, method: Method, path: RawPath, headers: Headers, agent: false }, (res) => {
            const Chunks: Buffer[] = [];
            res.on("data", (Chunk: Buffer) => Chunks.push(Chunk));
            res.on("end", () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(Chunks) }));
            res.on("error", reject);
        });
        Req.on("error", reject);
        Req.end();
    });
}

// Starts a download and stops reading after the headers, so the server's stream stays open (it
// stalls on backpressure). destroy() hangs up.
export function OpenStalled(Port: number, RawPath: string, Headers: Record<string, string>): Promise<{ status: number; destroy: () => void }> {
    return new Promise((resolve, reject) => {
        const Req = http.request({ host: "127.0.0.1", port: Port, method: "GET", path: RawPath, headers: Headers, agent: false }, (res) => {
            res.pause();
            resolve({ status: res.statusCode ?? 0, destroy: () => { res.destroy(); Req.destroy(); } });
        });
        Req.on("error", (error) => {
            if((error as NodeJS.ErrnoException).code !== "ECONNRESET"){
                reject(error);
            }
        });
        Req.end();
    });
}

export async function WaitFor(Condition: () => boolean | Promise<boolean>, TimeoutMs = 5000, What = "condition"){
    const Until = Date.now() + TimeoutMs;
    while(Date.now() < Until){
        if(await Condition()){
            return;
        }
        await new Promise((resolve) => setTimeout(resolve, 25));
    }
    throw new Error(`timed out waiting for ${What}`);
}

// A published runtime the way ReforgedHost's publish-runtime writes it.
export function Publish(Dir: string, Seed = 1, Version = "20261014-1a2b3c4d"): { dll: Buffer; loader: Buffer } {
    const Dll = Buffer.alloc(5000 + Seed, Seed);
    const Loader = Buffer.alloc(3000 + Seed, Seed + 1);
    fs.writeFileSync(path.join(Dir, "MysticParadox.dll"), Dll);
    fs.writeFileSync(path.join(Dir, "winmm.dll"), Loader);
    fs.writeFileSync(path.join(Dir, "runtime.json"), JSON.stringify({
        version: Version,
        build: { changelist: 647472 },
        files: [
            { name: "winmm.dll", size: Loader.length, sha256: Sha(Loader) },
            { name: "MysticParadox.dll", size: Dll.length, sha256: Sha(Dll) },
        ],
    }));
    return { dll: Dll, loader: Loader };
}

function Sha(Data: Buffer): string {
    return createHash("sha256").update(Data).digest("hex");
}
