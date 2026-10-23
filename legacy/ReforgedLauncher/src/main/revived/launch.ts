// The 1.4.4 command line, as Dauntless Revived's launcher 0.1.35 builds it (src/main/launch.ts there).
// The first argument is the server address, which UndauntedInternalServer.dll reads; the account key
// goes as -AUTH_PASSWORD and is never logged (../launch.ts maskArgs).

import { existsSync } from "node:fs";
import path from "node:path";
import { isValidHost } from "../../shared/invite";
import { isPlausibleAccountKey } from "../../shared/username";
import { DIRECTX_FILES } from "./constants";

export interface RevivedLaunchSettings {
  host: string;
  port: number;
  key: string;
  windowed: boolean;
  gameLocale?: string; // -epiclocale; English when left out
}

export const REVIVED_FIXED_ARGS = [
  "-AUTH_LOGIN=unused",
  "-AUTH_TYPE=exchangecode",
  "-epicapp=appidlol",
  "-epicenv=Prod",
  "-EpicPortal",
  "-epicusername=usernamelol",
  "-epicuserid=useridlol",
  "-epiclocale=en-US",
  "-epicsandboxid=sandboxidlol",
  "-epicdeploymentid=deploymentidlol",
  // The pinned 1.4.4 executable supports this client-mode switch; private servers do not use EAC.
  "-NoEAC",
];

const GAME_LOCALES = ["en-US", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "pt-BR", "ru-RU"];

export function buildRevivedArgs(s: RevivedLaunchSettings): string[] {
  if (!isValidHost(s.host)) throw new Error("invalid host");
  if (!Number.isInteger(s.port) || s.port < 1 || s.port > 65535) throw new Error("invalid port");
  if (!isPlausibleAccountKey(s.key)) throw new Error("invalid key");
  const locale = s.gameLocale && GAME_LOCALES.includes(s.gameLocale) ? s.gameLocale : "en-US";
  const fixed = REVIVED_FIXED_ARGS.map((a) => (a.startsWith("-epiclocale=") ? `-epiclocale=${locale}` : a));
  const args = [`${s.host}:${s.port}`, `-AUTH_PASSWORD=${s.key}`, ...fixed];
  if (s.windowed) args.push("-windowed", "-ResX=1280", "-ResY=720");
  return args;
}

// The DirectX June 2010 files the 1.4.4 client needs (Windows only).
export function missingDirectX(env: NodeJS.ProcessEnv = process.env, exists: (p: string) => boolean = existsSync, platform: NodeJS.Platform = process.platform): string[] {
  if (platform !== "win32") return [];
  return DIRECTX_FILES.filter((file) => !exists(path.win32.join(env.WINDIR ?? env.SystemRoot ?? "C:\\Windows", "System32", file)));
}
