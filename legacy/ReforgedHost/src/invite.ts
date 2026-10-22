// Invites: a code from the backend (its local admin route) wrapped in the link the launcher reads.
// The link format is the launcher's (legacy/ReforgedLauncher/src/shared/invite.ts), the same as Dauntless
// Revived's with its own scheme:
//   private  reforged-revival://join?v=1&host=<Tailscale IP>&port=61000&code=..&name=..[&share=..]
//   public   reforged-revival://join?v=2&mode=public&host=<IP or DNS>&port=443&fp=<cert SHA-256>&code=..&name=..

import { createHash, X509Certificate } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import tls from "node:tls";
import { PORTS } from "./paths";
import { gatewayPort, type HostConfig } from "./hostconfig";

export const INVITE_SCHEME = "reforged-revival";

export interface InviteParts {
    mode: "private" | "public";
    host: string;
    port: number;
    code: string;
    name: string;
    fp: string | null;
    share: string | null;
}

export function formatInvite(i: InviteParts): string {
    const parts: string[] = [];
    if (i.mode === "public") {
        if (i.fp === null || !/^[0-9a-f]{64}$/.test(i.fp)) throw new Error("public invites need the certificate fingerprint");
        parts.push("v=2", "mode=public", `host=${encodeURIComponent(i.host)}`, `port=${i.port}`, `fp=${i.fp}`);
    } else {
        parts.push("v=1", `host=${encodeURIComponent(i.host)}`, `port=${i.port}`);
    }
    parts.push(`code=${encodeURIComponent(i.code)}`, `name=${encodeURIComponent(i.name)}`);
    if (i.mode === "private" && i.share) parts.push(`share=${encodeURIComponent(i.share)}`);
    return `${INVITE_SCHEME}://join?${parts.join("&")}`;
}

// SHA-256 of the certificate's DER bytes: the invite's fp.
export function certificateFingerprint(certFile: string): string {
    const cert = new X509Certificate(fs.readFileSync(certFile));
    return createHash("sha256").update(cert.raw).digest("hex");
}

// The SHA-256 of the certificate whatever listens on host:port presents (the launcher pins it), or
// null when nothing answers with TLS.
export function servedFingerprint(port: number, host = "127.0.0.1", timeoutMs = 4000): Promise<string | null> {
    return new Promise((resolve) => {
        const socket = tls.connect({ host, port, rejectUnauthorized: false, timeout: timeoutMs }, () => {
            const cert = socket.getPeerX509Certificate();
            socket.destroy();
            resolve(cert ? createHash("sha256").update(cert.raw).digest("hex") : null);
        });
        socket.once("timeout", () => {
            socket.destroy();
            resolve(null);
        });
        socket.once("error", () => resolve(null));
    });
}

export function inviteFor(c: HostConfig, code: string, fp: string | null): string {
    return formatInvite({
        mode: c.mode,
        host: c.host,
        port: gatewayPort(c),
        code,
        name: c.name,
        fp: c.mode === "public" ? fp : null,
        share: c.mode === "private" ? c.shareUrl : null,
    });
}

function backendRequest(method: string, urlPath: string, adminKey: string, body: unknown | undefined, port: number): Promise<{ status: number; json: unknown }> {
    return new Promise((resolve, reject) => {
        const data = body === undefined ? undefined : Buffer.from(JSON.stringify(body));
        const req = http.request({
            host: "127.0.0.1",
            port,
            method,
            path: urlPath,
            agent: false,
            timeout: 10000,
            headers: { "x-reforged-admin-key": adminKey, ...(data ? { "content-type": "application/json", "content-length": String(data.length) } : {}) },
        }, (res) => {
            let text = "";
            res.on("data", (c) => (text += c));
            res.on("end", () => {
                let json: unknown = null;
                try {
                    json = JSON.parse(text);
                } catch {
                    json = null;
                }
                resolve({ status: res.statusCode ?? 0, json });
            });
        });
        req.on("timeout", () => req.destroy(new Error("the backend did not answer")));
        req.on("error", reject);
        req.end(data);
    });
}

// Asks the running backend for a new invite code (admin routes answer only on this machine).
export async function createInviteCode(adminKey: string, opts: { uses: number; infinite?: boolean; note?: string }, port: number = PORTS.backend): Promise<string> {
    let r: { status: number; json: unknown };
    try {
        r = await backendRequest("POST", "/reforged/api/CreateInvite", adminKey, { uses: opts.uses, infinite: opts.infinite === true, ...(opts.note ? { name: opts.note } : {}) }, port);
    } catch (e) {
        throw new Error(`the backend is not running on 127.0.0.1:${port} (start the server with npm run start first): ${e instanceof Error ? e.message : String(e)}`);
    }
    const code = (r.json as { code?: unknown } | null)?.code;
    if (r.status !== 200 || typeof code !== "string") {
        throw new Error(r.status === 403 ? "the backend refused the admin key (run npm run setup again so its env file and secrets.env agree)" : `the backend answered ${r.status}`);
    }
    return code;
}

export async function listInviteCodes(adminKey: string, port: number = PORTS.backend): Promise<unknown> {
    const r = await backendRequest("GET", "/reforged/api/InviteCodes", adminKey, undefined, port);
    if (r.status !== 200) throw new Error(`the backend answered ${r.status}`);
    return r.json;
}
