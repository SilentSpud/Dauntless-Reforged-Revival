// A small logger that can never write an account key. Every line goes through redact(), which
// removes registered secrets and anything shaped like a key or a -AUTH_PASSWORD= argument.

import { appendFileSync, mkdirSync, renameSync, statSync } from "node:fs";
import path from "node:path";

const secrets = new Set<string>();

export function addSecret(value: string): void {
  if (typeof value === "string" && value.length >= 6) secrets.add(value);
  if (typeof value === "string" && /^RRK_.{16,}$/.test(value)) secrets.add(value.slice(4));
}

export function removeSecret(value: string): void {
  secrets.delete(value);
  if (typeof value === "string" && value.startsWith("RRK_")) secrets.delete(value.slice(4));
}

export function redact(text: string): string {
  let out = text;
  for (const s of secrets) out = out.split(s).join("<hidden>");
  out = out.replace(/RRK_[A-Za-z0-9_-]+/g, "RRK_<hidden>");
  out = out.replace(/(-AUTH_PASSWORD=)[^\s"']*/gi, "$1<hidden>");
  out = out.replace(/(x-reforged-user-api-key["']?\s*[:=]\s*["']?)[^\s"',}]+/gi, "$1<hidden>");
  return out;
}

type Sink = (line: string) => void;

let sink: Sink = (line) => {
  // eslint-disable-next-line no-console
  console.log(line);
};

const MAX_LOG_BYTES = 2 * 1024 * 1024;

export function logToFile(dir: string): void {
  mkdirSync(dir, { recursive: true });
  const file = path.join(dir, "launcher.log");
  sink = (line) => {
    try {
      try {
        if (statSync(file).size > MAX_LOG_BYTES) renameSync(file, path.join(dir, "launcher.old.log"));
      } catch {
        /* no file yet */
      }
      appendFileSync(file, line + "\n");
    } catch {
      /* logging must never break the launcher */
    }
  };
}

export function setSink(s: Sink): void {
  sink = s;
}

function write(level: string, message: string): void {
  sink(redact(`${new Date().toISOString()} ${level} ${message}`));
}

export const log = {
  info: (message: string) => write("INFO", message),
  warn: (message: string) => write("WARN", message),
  error: (message: string) => write("ERROR", message),
};

export function describeError(e: unknown): string {
  if (e instanceof Error) return redact(`${e.name}: ${e.message}`);
  return redact(String(e));
}
