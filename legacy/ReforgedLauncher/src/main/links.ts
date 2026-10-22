// Which links the launcher may open in the browser. The renderer never passes a URL: it names a
// target (an ExternalTarget), the main process picks the URL and checks it here before
// shell.openExternal.

import type { ExternalTarget } from "../shared/types";
import {
  DUMPER7_URL,
  DUMPER7_ZIP_URL,
  GIT_URL,
  INSTALLING_GUIDE_URL,
  LEGENDARY_URL,
  MONGODB_URL,
  MYSTIC_URL,
  NODEJS_URL,
  PROJECT_DOCS_HOSTING_URL,
  PROJECT_DOCS_PLAYERS_URL,
  PROJECT_DOCS_SDK_URL,
  PROJECT_ISSUES_URL,
  PROJECT_LICENSE_URL,
  PROJECT_URL,
  REVIVED_URL,
  RUST_INSTALLER_URL,
  RUST_URL,
  TAILSCALE_DOWNLOAD_URL,
  UNDAUNTED_URL,
  VC_REDIST_URL,
  VISUAL_STUDIO_INSTALLER_URL,
  VISUAL_STUDIO_URL,
} from "./constants";

// The targets that always open one fixed URL. The other two come from the server side: the
// Tailscale share link in a private invite, and the host's own source link.
export type FixedTarget = Exclude<ExternalTarget, "tailscale_share" | "server_source">;

export const FIXED_LINKS: Readonly<Record<FixedTarget, string>> = Object.freeze({
  tailscale_download: TAILSCALE_DOWNLOAD_URL,
  vc_redist: VC_REDIST_URL,
  project_source: PROJECT_URL,
  project_license: PROJECT_LICENSE_URL,
  project_issues: PROJECT_ISSUES_URL,
  project_docs_players: PROJECT_DOCS_PLAYERS_URL,
  project_docs_hosting: PROJECT_DOCS_HOSTING_URL,
  project_docs_sdk: PROJECT_DOCS_SDK_URL,
  installing_guide: INSTALLING_GUIDE_URL,
  legendary: LEGENDARY_URL,
  dumper7: DUMPER7_URL,
  visual_studio: VISUAL_STUDIO_URL,
  rust: RUST_URL,
  nodejs: NODEJS_URL,
  mongodb: MONGODB_URL,
  git: GIT_URL,
  visual_studio_installer: VISUAL_STUDIO_INSTALLER_URL,
  rust_installer: RUST_INSTALLER_URL,
  dumper7_zip: DUMPER7_ZIP_URL,
  revived_source: REVIVED_URL,
  mystic_source: MYSTIC_URL,
  undaunted_source: UNDAUNTED_URL,
});

// The URL of a fixed target, or null for anything else (a raw URL, an unknown name, "__proto__").
export function fixedLinkUrl(target: unknown): string | null {
  if (typeof target !== "string" || !Object.prototype.hasOwnProperty.call(FIXED_LINKS, target)) return null;
  return FIXED_LINKS[target as FixedTarget];
}

const FIXED_URLS: ReadonlySet<string> = new Set(Object.values(FIXED_LINKS));

// Plain https without user info. An explicit port only where the caller allows one.
function parse(url: string, allowPort = false): URL | null {
  if (typeof url !== "string" || url.length > 1024) return null;
  try {
    const u = new URL(url);
    if (u.protocol !== "https:" || u.username !== "" || u.password !== "") return null;
    if (u.port !== "" && !allowPort) return null;
    return u;
  } catch {
    return null;
  }
}

export function isAllowedStaticUrl(url: string): boolean {
  const u = parse(url);
  if (!u) return false;
  if (FIXED_URLS.has(url)) return true;
  // A private invite's Tailscale share link.
  return u.hostname === "login.tailscale.com";
}

// The host's own source link (from ServerStatus.sourceUrl) is allowed as long as it is plain https,
// the same rule the status check applies (cleanHttpsUrl in src/shared/status.ts).
export function isAllowedExternalUrl(url: string, hostSourceUrl: string | null): boolean {
  if (isAllowedStaticUrl(url)) return true;
  if (hostSourceUrl !== null && url === hostSourceUrl) return parse(url, true) !== null;
  return false;
}
