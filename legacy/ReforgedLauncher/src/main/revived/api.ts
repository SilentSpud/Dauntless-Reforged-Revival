// The calls Dauntless Revived's launcher 0.1.35 makes to its servers besides the shared launcher API
// (ServerStatus, Register and GetUserInfo are in ../hostapi.ts, with REVIVED_FLAVOR): the content
// server's game manifest, the hunt region (autoregion.ts there) and the end of a play session.

import { createConnection } from "node:net";
import { performance } from "node:perf_hooks";
import { parseJsonBody, request, type Endpoint } from "../http";
import { keyHeaders, REVIVED_FLAVOR } from "../hostapi";
import { isValidHost } from "../../shared/invite";

const API = REVIVED_FLAVOR.base;

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

// Public mode: is the host's content server behind the gateway? (HEAD /content/v1/manifest)
export async function probeContent(ep: Endpoint): Promise<boolean> {
  try {
    const res = await request(ep, "/content/v1/manifest", { method: "HEAD", timeoutMs: 8000, maxBytes: 1024 });
    return res.status === 200;
  } catch {
    return false;
  }
}

export async function fetchContentManifest(ep: Endpoint): Promise<unknown | null> {
  try {
    const res = await request(ep, "/content/v1/manifest", { timeoutMs: 20000, maxBytes: 8 * 1024 * 1024 });
    return res.status === 200 ? parseJsonBody(res) ?? null : null;
  } catch {
    return null;
  }
}

export async function endPlaying(ep: Endpoint, key: string): Promise<void> {
  await request(ep, `${API}/PlayingEnded`, { method: "POST", headers: keyHeaders(ep, key, REVIVED_FLAVOR), timeoutMs: 5000, maxBytes: 4096 });
}

// ---------------------------------------------------------------- hunt region

export type Region = "main" | "ger" | "us";
const REGIONS: readonly Region[] = ["main", "ger", "us"];
export type RegionProbe = { region: Region; host: string; port: number };

export function tcpLatency(probe: RegionProbe): Promise<number> {
  return new Promise((resolve) => {
    const start = performance.now();
    const socket = createConnection({ host: probe.host, port: probe.port });
    let done = false;
    const finish = (value: number) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      socket.destroy();
      resolve(value);
    };
    const timer = setTimeout(() => finish(Infinity), 1500);
    socket.setTimeout(1500, () => finish(Infinity));
    socket.once("error", () => finish(Infinity));
    socket.once("connect", () => finish(performance.now() - start));
  });
}

// Two connects to each region (the first warms DNS and routes); the fastest answer wins.
export async function fastestRegion(probes: RegionProbe[], measure = tcpLatency): Promise<Region | undefined> {
  const readings = await Promise.all(
    probes.map(async (probe) => {
      await measure(probe).catch(() => Infinity);
      const latency = await measure(probe).catch(() => Infinity);
      return { region: probe.region, latency };
    }),
  );
  return readings.filter((r) => Number.isFinite(r.latency) && r.latency >= 0).sort((a, b) => a.latency - b.latency)[0]?.region;
}

// The server lists its hunt regions (GET HuntRegion); the closest one is picked, as 0.1.35 does with
// its default "auto". A server without regions answers 404: nothing to choose.
export async function chooseClosestRegion(ep: Endpoint, key: string, measure = tcpLatency): Promise<Region | undefined> {
  try {
    const res = await request(ep, `${API}/HuntRegion`, { headers: keyHeaders(ep, key, REVIVED_FLAVOR), timeoutMs: 5000, maxBytes: 8192 });
    if (res.status !== 200) return undefined;
    const body = parseJsonBody(res);
    if (!isObject(body)) return undefined;
    const probes: RegionProbe[] = [{ region: "main", host: ep.host, port: ep.port }];
    if (Array.isArray(body.probes)) {
      for (const c of body.probes.slice(0, 3)) {
        if (!isObject(c) || !REGIONS.includes(c.region as Region) || typeof c.host !== "string" || !isValidHost(c.host) || !Number.isInteger(c.port) || (c.port as number) < 1 || (c.port as number) > 65535) continue;
        const i = probes.findIndex((p) => p.region === c.region);
        if (i >= 0) probes.splice(i, 1);
        probes.push({ region: c.region as Region, host: c.host, port: c.port as number });
      }
    }
    return (await fastestRegion(probes, measure)) ?? (REGIONS.includes(body.region as Region) ? (body.region as Region) : undefined);
  } catch {
    return undefined;
  }
}

export async function saveHuntRegion(ep: Endpoint, key: string, region: Region): Promise<boolean> {
  const res = await request(ep, `${API}/HuntRegion`, { method: "POST", headers: keyHeaders(ep, key, REVIVED_FLAVOR), body: JSON.stringify({ region }), timeoutMs: 10000, maxBytes: 4096 });
  const body = parseJsonBody(res);
  return res.status === 200 && isObject(body) && body.region === region;
}
