// The launcher's page. It draws the Snapshots the main process sends and turns clicks into the
// small set of calls in preload.ts. It has no network access of its own: news, art and status
// all come through the main process. The structure follows Dauntless Revived's launcher page.

import { $, forceRender, githubMark, h, icon, renderRegion, type IconName } from "./dom";
import newsBadge from "./brand/news-badge.svg";
import { buildScene } from "./scene";
import { isStringKey, translate, type StringKey } from "../shared/i18n";
import { localized, PROJECTS, SOFTWARE, type CreditPerson, type CreditRole } from "../shared/credits";
import { parseInvite } from "../shared/invite";
import { checkUsername, extractAccountKey } from "../shared/username";
import { graphicsKey, hostSteps, primaryButton, shortHash, STEP_KEYS, stepIndex, taskView, whereKey, type StepState } from "../shared/ui-model";
import { formatBytes, formatDate, formatDuration, formatPercentText, formatRunningTime } from "../shared/format";
import { sortInstances, type InstanceKind, type ServerStatus, type StatusInstance } from "../shared/status";
import {
  GAME_LANGUAGES,
  GRAPHICS_PRESETS,
  LANGUAGES,
  MAX_MULTIPLIER,
  RUNTIME_PROFILES,
  type Branding,
  type ExternalTarget,
  type GameLanguage,
  type GraphicsPreset,
  type HostAction,
  type HostFolder,
  type HostKit,
  type HostServerView,
  type Language,
  type Requirement,
  type RequirementId,
  type LauncherError,
  type NewsItem,
  type RuntimeProfile,
  type Snapshot,
  type TaskProgress,
} from "../shared/types";

type View = "play" | "news" | "server" | "host" | "settings" | "credits";
const VIEWS: readonly View[] = ["play", "news", "server", "host", "settings", "credits"];
type Modal =
  | { kind: "invite"; link: string; name: string; host: string; mode: "public" | "private"; fp: string | null }
  // A public invite whose certificate is not the one this PC's key for that server belongs to.
  | { kind: "cert_changed"; link: string; name: string; host: string; oldFp: string | null; newFp: string }
  | { kind: "server_moved"; link: string; name: string; oldHost: string; newHost: string }
  | { kind: "trust_runtime" }
  | { kind: "logout" }
  | { kind: "leave" }
  | { kind: "remove_server"; id: string; name: string };

const api = window.launcher;
const ART_URL = /^rr-art:\/\/bg\/[0-9a-f]{64}$/;
const TIME_LOCALES: Record<Language, string> = { en: "en-GB", "pt-BR": "pt-BR" };
const LANGUAGE_NAMES: Record<Language, string> = { en: "English", "pt-BR": "Português (Brasil)" };
const GAME_LANGUAGE_NAMES: Record<Exclude<GameLanguage, "auto">, string> = { "en-US": "English", "de-DE": "Deutsch", "es-ES": "Español", "fr-FR": "Français", "it-IT": "Italiano", "ja-JP": "日本語", "pt-BR": "Português (Brasil)", "ru-RU": "Русский" };

type SettingsTab = "game" | "graphics" | "account" | "launcher";
const SETTINGS_TABS: SettingsTab[] = ["game", "graphics", "account", "launcher"];
const SETTINGS_TAB_ICONS: Record<SettingsTab, IconName> = { game: "gamepad", graphics: "monitor", account: "user", launcher: "settings" };

const state = {
  snap: null as Snapshot | null,
  task: null as TaskProgress | null,
  host: null as HostKit | null,
  hostServer: null as HostServerView | null,
  hostTab: "server" as "server" | "gamedata" | "build" | "requirements",
  consoleOpen: false,
  setupMode: null as "private" | "public" | null,
  copied: null as string | null,
  reqOpen: new Set<string>(), // the Requirements tab's open and closed install guides
  reqClosed: new Set<string>(),
  lang: "en" as Language,
  view: "play" as View,
  inviteText: "",
  revealed: new Set<string>(), // streamer mode: the values shown with the eye, until the launcher closes
  username: "",
  keyFormOpen: false,
  pathFormOpen: false,
  news: [] as NewsItem[],
  newsSeen: false,
  serverMenu: false, // the rail's server switcher is open
  settingsTab: "game" as SettingsTab,
  branding: { backgrounds: [], accent: null } as Branding,
  extrasFor: "",
  extrasAt: 0,
  localError: null as LauncherError | null,
  modal: null as Modal | null,
  modalReturnFocus: null as HTMLElement | null,
  // Where Escape on the Credits page goes back to: the page and the button that opened it.
  creditsReturn: null as { view: View; focus: HTMLElement | null } | null,
  maximized: false,
  artIndex: -1,
  artTimer: 0 as number,
  artLayer: 0,
};

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

function t(key: StringKey, vars?: Record<string, string | number>): string {
  return translate(state.lang, key, vars);
}

function tk(key: string, vars?: Record<string, string | number>): string {
  return isStringKey(key) ? t(key, vars) : key;
}

// ------------------------------------------------------------------ persistent inputs

const inviteInput = h("textarea", {
  class: "textarea",
  id: "invite-input",
  rows: 3,
  spellcheck: "false",
  autocomplete: "off",
  "data-fk": "invite",
  "aria-describedby": "invite-hint",
});
const inviteHint = h("div", { class: "hint", id: "invite-hint", "aria-live": "polite" });
const invitePreview = h("div", { class: "preview", hidden: true });

const usernameInput = h("input", {
  class: "input input-big",
  id: "username-input",
  type: "text",
  maxlength: 16,
  spellcheck: "false",
  autocomplete: "off",
  "data-fk": "username",
  "aria-describedby": "username-hint",
});
const usernameHint = h("div", { class: "hint", id: "username-hint", "aria-live": "polite" });

const keyInput = h("input", {
  class: "input",
  id: "key-input",
  type: "password",
  spellcheck: "false",
  autocomplete: "off",
  "data-fk": "key",
});

// The game folder, pasted. The main process checks it again.
const gamePathInput = h("input", {
  class: "input",
  id: "game-path",
  type: "text",
  maxlength: 260,
  spellcheck: "false",
  autocomplete: "off",
  "data-fk": "game-path",
});
gamePathInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    void useGamePath();
  }
});

inviteInput.addEventListener("input", () => {
  state.inviteText = inviteInput.value;
  updateInviteFeedback();
  renderActionBar();
});
inviteInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    void onPrimary();
  }
});
usernameInput.addEventListener("input", () => {
  state.username = usernameInput.value;
  updateUsernameFeedback();
  renderActionBar();
});
usernameInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    void onPrimary();
  }
});
keyInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    void useKey();
  }
});

function clearInviteInput(): void {
  state.inviteText = "";
  inviteInput.value = "";
  updateInviteFeedback();
}

function formState() {
  return {
    inviteValid: parseInvite(state.inviteText).ok,
    usernameValid: checkUsername(state.username.trim()) === "ok",
  };
}

function groupedFingerprint(fp: string): string {
  return fp.match(/.{1,4}/g)?.join(" ") ?? fp;
}

// The first 16 hex digits, enough to compare with what the host shows.
function shortFingerprint(fp: string): string {
  return `${groupedFingerprint(fp.slice(0, 16))}…`;
}

function modeBadge(mode: "public" | "private"): HTMLElement {
  return mode === "public"
    ? h("span", { class: "badge badge-public" }, icon("shield"), t("mode_public"))
    : h("span", { class: "badge badge-private" }, icon("tunnel"), t("mode_private"));
}

function updateInviteFeedback(): void {
  const text = state.inviteText.trim();
  invitePreview.replaceChildren();
  if (text.length === 0) {
    invitePreview.hidden = true;
    inviteInput.removeAttribute("aria-invalid");
    inviteHint.className = "hint";
    inviteHint.replaceChildren(t("join_hint"));
    return;
  }
  const r = parseInvite(text);
  if (r.ok) {
    inviteInput.removeAttribute("aria-invalid");
    invitePreview.hidden = false;
    invitePreview.append(
      icon("check"),
      h(
        "div",
        { class: "preview-text" },
        h("div", { class: "preview-name" }, r.invite.name),
        h("div", { class: "preview-host" }, secret(`${r.invite.host}:${r.invite.port}`, "invite-host")),
        r.invite.fp ? h("div", { class: "preview-host mono" }, t("link_fingerprint", { fp: hideText(shortFingerprint(r.invite.fp)) })) : null,
      ),
      h("span", {}, modeBadge(r.invite.mode)),
    );
    inviteHint.className = "hint hint-ok";
    inviteHint.replaceChildren(t(r.invite.mode === "public" ? "join_ok_public" : "join_ok_private"));
  } else {
    invitePreview.hidden = true;
    inviteInput.setAttribute("aria-invalid", "true");
    inviteHint.className = "hint hint-bad";
    inviteHint.replaceChildren(icon("warning"), tk(`invite_err_${r.error}`));
  }
}

function updateUsernameFeedback(): void {
  const check = checkUsername(state.username.trim());
  usernameHint.className = check === "ok" ? "hint hint-ok" : state.username.length === 0 ? "hint" : "hint hint-bad";
  if (check === "ok") usernameHint.replaceChildren(icon("check"), t("reg_check_ok"));
  else usernameHint.replaceChildren(tk(`reg_check_${check}`));
  if (check !== "ok" && state.username.length > 0) usernameInput.setAttribute("aria-invalid", "true");
  else usernameInput.removeAttribute("aria-invalid");
}

// ------------------------------------------------------------------ streamer mode

// Streamer mode keeps what could identify a server or the PC off the screen: addresses, certificate
// fingerprints, the invite text, folders (they hold the Windows user name) and other players'
// names. A value that the player may need gets an eye button to show it for a moment.
function streamer(): boolean {
  return state.snap?.settings.streamerMode === true;
}

function hideText(text: string): string {
  return streamer() ? "••••••" : text;
}

// A hidden value is never on screen: it is swapped for random letters and digits of the same length,
// and that filler is what gets blurred, so nothing can be recovered from a stream or a screenshot.
const fillers = new Map<string, string>();
function filler(text: string): string {
  let out = fillers.get(text);
  if (out === undefined) {
    const chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    const bytes = crypto.getRandomValues(new Uint8Array(Array.from(text).length));
    out = Array.from(bytes, (v) => chars[v % chars.length]).join("");
    fillers.set(text, out);
  }
  return out;
}

function blurred(text: string, cls = ""): HTMLElement {
  if (!streamer()) return h("span", { class: cls || undefined }, text);
  return h("span", { class: `${cls} secret-blur`.trim(), "aria-hidden": "true" }, filler(text));
}

function secret(text: string, key: string, cls = ""): HTMLElement {
  if (!streamer()) return h("span", { class: cls || undefined }, text);
  const shown = state.revealed.has(key);
  const eye = h("button", { type: "button", class: "secret-eye", title: t(shown ? "streamer_hide" : "streamer_show"), "aria-label": t(shown ? "streamer_hide" : "streamer_show"), "aria-pressed": shown ? "true" : "false" }, icon(shown ? "eye_off" : "eye"));
  eye.addEventListener("click", () => {
    if (shown) state.revealed.delete(key);
    else state.revealed.add(key);
    refreshSecrets();
  });
  return h("span", { class: "secret" }, h("span", { class: `${cls} ${shown ? "" : "secret-blur"}`.trim() || undefined, "aria-hidden": shown ? undefined : "true" }, shown ? text : filler(text)), eye);
}

function refreshSecrets(): void {
  document.documentElement.classList.toggle("streamer", streamer());
  inviteInput.classList.toggle("masked", streamer());
  for (const sel of ["#view-play", "#view-server", "#view-settings", "#view-host", "#server-panel", "#account-chip"]) forceRender($(sel));
  updateInviteFeedback();
  renderAll();
}

// ------------------------------------------------------------------ small builders

function card(cls: string, ...children: Parameters<typeof h>[2][]): HTMLElement {
  return h("div", { class: `card ${cls}`.trim() }, ...children);
}

function linkButton(label: string, onClick: () => void, opts: { icon?: IconName; fk?: string; disabled?: boolean } = {}): HTMLButtonElement {
  const b = h("button", { type: "button", class: "link-btn", "data-fk": opts.fk, disabled: opts.disabled }, opts.icon ? icon(opts.icon) : null, label);
  b.addEventListener("click", onClick);
  return b;
}

// A risky action (btn-danger) always shows the warning sign, so it does not rely on its colour alone.
function button(label: string, onClick: () => void, opts: { cls?: string; icon?: IconName; fk?: string; disabled?: boolean } = {}): HTMLButtonElement {
  const glyph = opts.icon ?? (opts.cls === "btn-danger" ? "warning" : undefined);
  const b = h("button", { type: "button", class: `btn ${opts.cls ?? ""}`.trim(), "data-fk": opts.fk, disabled: opts.disabled }, glyph ? icon(glyph) : null, label);
  b.addEventListener("click", onClick);
  return b;
}

function stat(label: string, value: string, small = false): HTMLElement {
  return h("div", { class: "stat" }, h("span", { class: "stat-label" }, label), h("span", { class: `stat-value${small ? " small" : ""}`, title: value }, value));
}

function chip(stateName: StepState | "ok", text: string): HTMLElement {
  const cls = stateName === "done" || stateName === "ok" ? "chip chip-ok" : stateName === "warn" ? "chip chip-warn" : "chip chip-todo";
  return h("span", { class: cls }, icon(stateName === "done" || stateName === "ok" ? "check" : stateName === "warn" ? "warning" : "info"), text);
}

function serverName(): string {
  return state.snap?.status?.name ?? state.snap?.server?.name ?? "";
}

function open(target: ExternalTarget): void {
  void api.openExternal(target);
}

// Show an error from an action whose main-process side did not already report it.
async function report(r: { ok: boolean; error?: LauncherError }): Promise<void> {
  if (r.ok || !r.error || r.error.code === "cancelled") return;
  if (!(await api.getSnapshot()).lastError) state.localError = r.error;
  renderBanners();
}

function errorText(e: LauncherError): string {
  switch (e.code) {
    case "account_banned":
      return t("err_account_banned", { reason: e.detail || t("err_account_banned_noreason") });
    case "invite_invalid_format":
      return e.detail ? tk(`invite_err_${e.detail}`) : t("err_invite_invalid_format");
    case "username_invalid":
      return e.detail && e.detail !== "ok" ? tk(`reg_check_${e.detail}`) : t("err_username_invalid");
    case "disk_space":
      return t("err_disk_space");
    case "relay_port_busy":
      return t("err_relay_port_busy", { port: e.detail && /^\d+$/.test(e.detail) ? e.detail : "61100" });
    case "server_unreachable":
      return t(state.snap?.server?.mode === "public" ? "err_server_unreachable_public" : "err_server_unreachable");
    case "launch_failed":
      return `${t("err_launch_failed")}${e.detail ? ` (${e.detail})` : ""}`;
    case "host_failed":
      return e.detail && isStringKey(`host_err_${e.detail}`) ? tk(`host_err_${e.detail}`) : t("err_host_failed", { detail: e.detail ?? "" });
    case "local_runtime_invalid":
      return t("err_local_runtime_invalid");
    case "windows_runtime_missing":
      return `${t("err_windows_runtime_missing")}${e.detail ? ` (${e.detail})` : ""}`;
    default:
      return tk(`err_${e.code}`);
  }
}

// ------------------------------------------------------------------ play view

function vcWarning(): HTMLElement | null {
  return null;
}

// A Dauntless Revived server runs Dauntless 1.4.4: said next to its name wherever servers are listed.
function editionBadge(game: Snapshot["savedServers"][number]["game"]): HTMLElement | null {
  return game === "revived" ? h("span", { class: "badge badge-revived" }, icon("gamepad"), t("edition_revived")) : null;
}

function eyebrow(snap: Snapshot): HTMLElement {
  return h("div", { class: "eyebrow" }, snap.server ? serverName() : t("join_eyebrow"), snap.server ? modeBadge(snap.server.mode) : null, snap.server ? editionBadge(snap.server.game) : null);
}

function heading(title: string, text: string | null): HTMLElement[] {
  const out: HTMLElement[] = [h("h1", { class: "hero-title", id: "play-title" }, title)];
  if (text) out.push(h("p", { class: "hero-text" }, text));
  return out;
}

// Same rule as the Settings page: no switching servers while a task or the game runs.
function leaveLink(snap: Snapshot): HTMLButtonElement {
  return linkButton(t("connect_use_other"), () => showModal({ kind: "leave" }), { fk: "leave", disabled: snap.busy || snap.task !== null || snap.running.game });
}

async function switchTo(id: string): Promise<void> {
  const r = await api.switchServer(id);
  if (!r.ok && r.error && r.error.code !== "cancelled") {
    await report(r);
    return;
  }
  setView("play");
}

function savedServerSub(sv: Snapshot["savedServers"][number]): HTMLElement {
  const address = blurred(`${sv.host}:${sv.port}`);
  const edition = sv.game === "revived" ? `${t("edition_revived")} · ` : "";
  return sv.username ? h("span", {}, `${edition}${t("saved_account", { name: sv.username })} · `, address) : h("span", {}, edition, address);
}

function savedServersCard(snap: Snapshot): HTMLElement | null {
  const others = snap.savedServers.filter((sv) => !sv.current);
  if (others.length === 0) return null;
  const locked = snap.busy || snap.task !== null || snap.running.game;
  const rows = others.map((sv) =>
    h(
      "div",
      { class: "settings-row" },
      h("div", { class: "settings-row-text" }, h("span", { class: "settings-row-title" }, sv.name, " ", modeBadge(sv.mode)), h("span", { class: "settings-row-sub mono" }, savedServerSub(sv))),
      h(
        "div",
        { class: "settings-actions" },
        button(t("saved_join"), () => void switchTo(sv.id), { cls: "btn-primary", fk: `saved-${sv.id}`, disabled: locked }),
        linkButton(t("saved_remove"), () => showModal({ kind: "remove_server", id: sv.id, name: sv.name }), { icon: "close", fk: `saved-rm-${sv.id}`, disabled: locked }),
      ),
    ),
  );
  return card("", h("h2", { class: "card-title" }, t("saved_title")), h("p", { class: "card-text" }, t("saved_text")), ...rows);
}

function hostTeaser(): HTMLElement {
  return card(
    "host-teaser",
    h("h2", { class: "card-title" }, icon("anvil"), t("host_teaser_title")),
    h("p", { class: "card-text" }, t("host_teaser_text")),
    h(
      "div",
      { class: "card-row" },
      button(t("host_teaser_open"), () => {
        void api.setSettings({ hostTools: true }).then(() => setView("host"));
      }, { icon: "hammer", fk: "host-teaser" }),
      linkButton(t("host_teaser_docs"), () => open("project_docs_hosting"), { icon: "external", fk: "host-teaser-docs" }),
    ),
  );
}

function playJoin(snap: Snapshot): HTMLElement[] {
  const saved = savedServersCard(snap);
  return [
    h("div", { class: "eyebrow" }, t("join_eyebrow")),
    ...heading(t("join_title"), t("join_text")),
    ...(saved ? [saved] : []),
    card("", h("div", { class: "field" }, h("label", { class: "field-label", for: "invite-input" }, t("join_label")), inviteInput, invitePreview, inviteHint), h("p", { class: "small-print" }, t("join_next"))),
    ...(snap.settings.hostTools ? [] : [hostTeaser()]),
  ];
}

function playConnect(snap: Snapshot): HTMLElement[] {
  const sv = snap.server!;
  const name = serverName();
  const pub = sv.mode === "public";
  const lastChecked = snap.connect.lastCheckedAt ? t("connect_last_checked", { time: new Date(snap.connect.lastCheckedAt).toLocaleTimeString(TIME_LOCALES[state.lang]) }) : null;
  const foot = h("div", { class: "card-row" }, lastChecked ? h("span", { class: "hint" }, lastChecked) : null, leaveLink(snap));
  if (snap.connect.checking || (snap.connect.problem === null && snap.connect.lastCheckedAt === null)) {
    return [eyebrow(snap), ...heading(t("connect_checking_title", { name }), t(pub ? "connect_checking_text_public" : "connect_checking_text")), card("", h("div", { class: "card-row" }, h("div", { class: "spinner", role: "presentation" }), t("connect_checking_wait")))];
  }
  switch (snap.connect.problem) {
    case "tailscale_missing":
      return [
        eyebrow(snap),
        ...heading(t("connect_ts_title"), t("connect_ts_text", { name })),
        card(
          "",
          h(
            "ol",
            { class: "steps-list" },
            h("li", {}, h("div", {}, h("div", {}, t("connect_step_install")), linkButton(t("connect_btn_install_ts"), () => open("tailscale_download"), { icon: "external", fk: "ts-dl" }))),
            h("li", {}, h("div", {}, h("div", {}, t(sv.hasShare ? "connect_step_accept" : "connect_step_accept_nolink")), sv.hasShare ? linkButton(t("connect_btn_accept"), () => open("tailscale_share"), { icon: "external", fk: "ts-share" }) : null)),
            h("li", {}, t("connect_step_retry")),
          ),
        ),
        foot,
      ];
    case "cert_mismatch":
      return [
        eyebrow(snap),
        ...heading(t("connect_cert_title"), null),
        card("card-danger", h("h3", { class: "card-title" }, icon("shield"), t("connect_cert_subtitle")), h("p", { class: "card-text" }, t("connect_cert_text", { host: hideText(sv.host) })), h("p", { class: "card-text" }, t("connect_cert_next"))),
        foot,
      ];
    case "bad_answer":
      return [eyebrow(snap), ...heading(t("connect_bad_title"), t("connect_bad_text", { host: hideText(sv.host), port: sv.port })), foot];
    default:
      return [
        eyebrow(snap),
        ...heading(t("connect_unreach_title", { name }), t(pub ? "connect_unreach_text_public" : "connect_unreach_text", { host: hideText(sv.host) })),
        card(
          "",
          h(
            "ul",
            { class: "check-list" },
            ...(pub
              ? [h("li", {}, t("connect_check_internet")), h("li", {}, t("connect_check_server")), h("li", {}, t("connect_check_firewall"))]
              : [h("li", {}, t("connect_check_ts_on")), h("li", {}, t("connect_check_share")), h("li", {}, t("connect_check_server"))]),
          ),
        ),
        foot,
      ];
  }
}

async function useKey(): Promise<void> {
  const key = extractAccountKey(keyInput.value);
  if (!key) {
    state.localError = { code: "key_invalid" };
    renderBanners();
    return;
  }
  const r = await api.useExistingKey(key);
  if (r.ok) {
    keyInput.value = "";
    state.keyFormOpen = false;
    state.localError = null;
  } else if (!state.snap?.lastError) {
    state.localError = r.error;
  }
  renderAll();
}

async function useGamePath(): Promise<void> {
  if (!gamePathInput.value.trim()) {
    gamePathInput.focus();
    return;
  }
  const r = await api.useGamePath(gamePathInput.value);
  if (r.ok) {
    gamePathInput.value = "";
    state.pathFormOpen = false;
    state.localError = null;
  } else if (!state.snap?.lastError) {
    state.localError = r.error;
  }
  renderAll();
}

function playRegister(snap: Snapshot): HTMLElement[] {
  const name = serverName();
  const out: HTMLElement[] = [eyebrow(snap), ...heading(t("reg_title"), t("reg_text", { name }))];
  if (snap.lastError?.code === "key_rejected") {
    out.push(card("card-warn", h("h3", { class: "card-title" }, icon("key"), t("reg_rejected_title")), h("p", { class: "card-text" }, t("reg_rejected", { name }))));
  }
  if (snap.status?.registration === "NONE") {
    out.push(card("card-warn", h("h3", { class: "card-title" }, icon("warning"), t("reg_closed_title")), h("p", { class: "card-text" }, t("reg_closed"))));
  }
  const keyForm = state.keyFormOpen
    ? h(
        "div",
        { class: "field" },
        h("label", { class: "field-label", for: "key-input" }, t("reg_key_label")),
        keyInput,
        h("div", { class: "card-row" }, button(t("reg_key_use"), () => void useKey(), { cls: "btn-primary", icon: "key", fk: "key-use", disabled: snap.busy }), button(t("reg_key_file"), () => void api.importKeyFile().then(report), { icon: "folder", fk: "key-file", disabled: snap.busy })),
        h("p", { class: "small-print" }, t("reg_key_note")),
      )
    : null;
  out.push(
    card(
      "",
      h("div", { class: "field" }, h("label", { class: "field-label", for: "username-input" }, t("reg_label")), usernameInput, usernameHint),
      h("p", { class: "small-print" }, icon(snap.server?.hasPendingInvite ? "check" : "info"), " ", snap.server?.hasPendingInvite ? t("reg_invite_included") : t("reg_invite_missing")),
    ),
    card(
      "",
      linkButton(state.keyFormOpen ? t("reg_have_key_hide") : t("reg_have_key"), () => {
        state.keyFormOpen = !state.keyFormOpen;
        renderPlay();
        if (state.keyFormOpen) keyInput.focus();
      }, { icon: "key", fk: "key-toggle" }),
      keyForm,
    ),
  );
  return out;
}

function sourceName(source: string): string {
  return tk(`game_source_${source}`);
}

function gamePathForm(snap: Snapshot): HTMLElement {
  const form = state.pathFormOpen
    ? h(
        "div",
        { class: "field" },
        h("label", { class: "field-label", for: "game-path" }, t("locate_path_label")),
        gamePathInput,
        h("div", { class: "card-row" }, button(t("locate_path_use"), () => void useGamePath(), { cls: "btn-primary", icon: "check", fk: "path-use", disabled: snap.busy })),
        h("p", { class: "small-print" }, t("locate_path_note")),
      )
    : null;
  return card(
    "",
    h(
      "div",
      { class: "card-row" },
      button(t("locate_browse"), () => void api.chooseGameFolder().then(report), { icon: "folder", fk: "locate-browse", disabled: snap.busy }),
      linkButton(state.pathFormOpen ? t("locate_path_hide") : t("locate_path"), () => {
        state.pathFormOpen = !state.pathFormOpen;
        renderPlay();
        if (state.pathFormOpen) gamePathInput.focus();
      }, { icon: "copy", fk: "locate-path", disabled: snap.busy }),
    ),
    form,
  );
}

function detectedCard(snap: Snapshot): HTMLElement | null {
  const list = snap.game.detected;
  if (list.length === 0) return null;
  return card(
    "",
    h("h2", { class: "card-title" }, t("locate_found_title")),
    h(
      "ul",
      { class: "choice-list" },
      ...list.map((d, i) => {
        const current = snap.game.root !== null && d.root.toLowerCase() === snap.game.root.toLowerCase();
        return h(
          "li",
          { class: "choice" },
          icon("gamepad"),
          h("div", { class: "choice-text" }, h("span", { class: "choice-title" }, sourceName(d.source)), h("span", { class: "choice-sub" }, secret(d.root, `detected-${i}`)), d.version ? h("span", { class: "choice-sub" }, t("locate_found_version", { version: d.version })) : null),
          current ? h("span", { class: "chip chip-ok" }, icon("check"), t("locate_in_use")) : button(t("locate_use"), () => void api.useDetectedGame(i).then(report), { cls: "btn-primary", fk: `detected-${i}`, disabled: snap.busy }),
        );
      }),
    ),
  );
}

// Dauntless 1.4.4, for a Dauntless Revived server: where it is (or goes), and download or check it.
function playLocateRevived(snap: Snapshot): HTMLElement[] {
  const r = snap.revived!;
  const out: HTMLElement[] = [eyebrow(snap)];
  if (r.problem === "not_set") out.push(...heading(t("revived_download_title"), t("revived_download_text", { size: formatBytes(r.totalBytes, state.lang) })));
  else out.push(...heading(t("revived_check_title"), r.missingFiles > 0 ? t("revived_check_missing", { n: r.missingFiles }) : t("revived_check_text")));
  out.push(
    card(
      "",
      h("div", { class: "settings-row" }, h("div", { class: "settings-row-text" }, h("span", { class: "settings-row-title" }, r.problem === "not_set" ? t("revived_folder_target") : t("revived_folder_current")), h("span", { class: "settings-row-sub selectable" }, secret(r.root, "revived-root")))),
      h("div", { class: "card-row" }, button(t("revived_choose"), () => void api.revivedChooseFolder().then(report), { icon: "folder", fk: "revived-choose" })),
      h("p", { class: "small-print" }, t("revived_choose_hint")),
    ),
  );
  if (r.detected.length > 0 && !r.detected.some((d) => d.root === r.root)) {
    out.push(card("card-accent", h("h3", { class: "card-title" }, icon("check"), t("revived_detected_title")), ...r.detected.map((d, i) => h("div", { class: "card-row" }, h("span", { class: "selectable" }, secret(d.root, `revived-detected-${i}`)), button(t("locate_use"), () => void api.revivedUseDetected(i).then(report), { cls: "btn-primary", fk: `revived-detected-${i}`, disabled: snap.busy })))));
  }
  out.push(card("card-warn", h("p", { class: "card-text" }, icon("info"), " ", t("revived_about"))));
  return out;
}

function playLocate(snap: Snapshot): HTMLElement[] {
  if (snap.revived) return playLocateRevived(snap);
  const g = snap.game;
  const out: HTMLElement[] = [eyebrow(snap)];
  switch (g.problem) {
    case "wrong_version":
      out.push(...heading(t("locate_wrong_title"), t("locate_wrong_text")));
      out.push(
        card(
          "card-danger",
          h("h3", { class: "card-title" }, icon("warning"), t("locate_wrong_subtitle")),
          h("p", { class: "card-text" }, g.versionText ? t("locate_wrong_found", { version: g.versionText }) : t("locate_wrong_unknown")),
          h("p", { class: "card-text" }, t("locate_wrong_fix")),
          h("div", { class: "card-row" }, linkButton(t("locate_guide"), () => open("installing_guide"), { icon: "external", fk: "guide" })),
        ),
      );
      break;
    case "unverified":
      out.push(...heading(t("locate_verify_title"), t("locate_verify_text")));
      break;
    case "not_found":
      out.push(...heading(t("locate_missing_title"), t("locate_missing_text")));
      break;
    default:
      out.push(...heading(t("locate_title"), t("locate_text")));
      if (g.detected.length === 0) {
        out.push(
          card(
            "card-warn",
            h("h3", { class: "card-title" }, icon("info"), t("locate_none_title")),
            h("p", { class: "card-text" }, t("locate_none_text")),
            h("div", { class: "card-row" }, linkButton(t("locate_guide"), () => open("installing_guide"), { icon: "external", fk: "guide" }), linkButton(t("locate_legendary"), () => open("legendary"), { icon: "external", fk: "legendary" })),
          ),
        );
      }
  }
  if (g.root) out.push(card("", h("div", { class: "settings-row" }, h("div", { class: "settings-row-text" }, h("span", { class: "settings-row-title" }, t("locate_current")), h("span", { class: "settings-row-sub selectable" }, secret(g.root, "game-root"))))));
  const detected = detectedCard(snap);
  if (detected) out.push(detected);
  out.push(gamePathForm(snap));
  return out;
}

function runtimeFiles(snap: Snapshot): HTMLElement {
  return h(
    "ul",
    { class: "file-list" },
    ...snap.runtime.files.map((f) => h("li", {}, h("span", { class: "file-name" }, f.name), h("span", {}, formatBytes(f.size, state.lang)), h("span", { class: "file-hash", title: f.sha256 }, shortHash(f.sha256)))),
  );
}

function playRuntime(snap: Snapshot): HTMLElement[] {
  const name = serverName();
  const r = snap.runtime;
  const out: HTMLElement[] = [eyebrow(snap)];
  switch (r.state) {
    case "untrusted":
      out.push(...heading(t("runtime_trust_title"), t("runtime_trust_text", { name })));
      out.push(card("card-accent", h("h2", { class: "card-title" }, icon("shield"), t("runtime_files_title", { version: r.version ?? "?" })), h("p", { class: "card-text" }, t("runtime_trust_detail")), runtimeFiles(snap)));
      break;
    case "missing":
    case "outdated":
      out.push(...heading(t(r.state === "missing" ? "runtime_install_title" : "runtime_update_title"), t("runtime_install_text", { name })));
      out.push(card("", h("h2", { class: "card-title" }, t("runtime_files_title", { version: r.version ?? "?" })), runtimeFiles(snap)));
      break;
    default:
      out.push(...heading(t("runtime_none_title"), t("runtime_none_text", { name })));
      out.push(card("card-warn", h("h3", { class: "card-title" }, icon("anvil"), t("runtime_none_host_title")), h("p", { class: "card-text" }, t("runtime_none_host_text")), h("div", { class: "card-row" }, button(t("host_teaser_open"), () => void api.setSettings({ hostTools: true }).then(() => setView("host")), { icon: "hammer", fk: "runtime-host" }), linkButton(t("host_teaser_docs"), () => open("project_docs_sdk"), { icon: "external", fk: "runtime-docs" }))));
  }
  return out;
}

function playInstalling(snap: Snapshot): HTMLElement[] {
  const kind = state.task?.kind ?? snap.task?.kind ?? "verify";
  if (snap.revived) return [eyebrow(snap), ...heading(t(kind === "download" ? "revived_installing_download" : kind === "dlls" ? "revived_installing_dlls" : "revived_installing_verify"), t("revived_installing_text"))];
  return [eyebrow(snap), ...heading(kind === "runtime" ? t("installing_runtime_title") : t("installing_verify_title"), t(kind === "runtime" ? "installing_runtime_text" : "installing_verify_text"))];
}

function gameDataWarning(snap: Snapshot): HTMLElement | null {
  const kind = snap.status?.gameData;
  if (kind !== "synthetic" && kind !== "missing") return null;
  return card("card-warn", h("h3", { class: "card-title" }, icon("database"), t(kind === "synthetic" ? "gamedata_synthetic_title" : "gamedata_missing_title")), h("p", { class: "card-text" }, t(kind === "synthetic" ? "gamedata_synthetic_text" : "gamedata_missing_text")));
}

function newsTeaser(): HTMLElement | null {
  const item = state.news[0];
  if (!item) return null;
  return card("", h("div", { class: "news-teaser" }, item.date ? h("span", { class: "news-teaser-date" }, formatDate(item.date, state.lang)) : null, h("strong", {}, item.title), h("div", { class: "card-row" }, linkButton(t("news_read_more"), () => setView("news"), { fk: "news-more" }))));
}

function playReady(snap: Snapshot): HTMLElement[] {
  const user = snap.account.username;
  const out: HTMLElement[] = [eyebrow(snap), ...heading(user ? t("ready_title", { user }) : t("ready_title_nouser"), t("ready_text"))];
  const warning = gameDataWarning(snap);
  if (warning) out.push(warning);
  out.push(
    card(
      "",
      h(
        "div",
        { class: "card-row" },
        h("span", { class: "badge" }, icon("gamepad"), t(snap.revived ? "ready_build_revived" : "ready_build")),
        h("span", { class: "badge" }, icon("settings"), t("ready_graphics", { preset: t(graphicsKey(snap.settings.graphics)) })),
        h("span", { class: "badge" }, t(snap.settings.windowed ? "ready_windowed" : "ready_fullscreen")),
        snap.runtime.state === "local" ? h("span", { class: "badge badge-ember" }, icon("anvil"), t("ready_local_runtime")) : null,
        linkButton(t("ready_change"), () => setView("settings"), { fk: "ready-change" }),
        // Dauntless 1.4.4: every file hashed again, and anything damaged downloaded again.
        snap.revived ? linkButton(t("revived_repair"), () => void api.revivedInstall("repair").then(report), { icon: "refresh", fk: "revived-repair", disabled: snap.busy }) : null,
      ),
    ),
  );
  if (snap.account.offerBackup) {
    out.push(
      card(
        "card-accent",
        h("h3", { class: "card-title" }, t("backup_title")),
        h("p", { class: "card-text" }, t("backup_text")),
        h("div", { class: "card-row" }, button(t("backup_save"), () => void api.saveKeyBackup().then(report), { cls: "btn-primary", icon: "key", fk: "backup-save" }), button(t("backup_later"), () => void api.dismissBackupOffer(), { fk: "backup-later" })),
      ),
    );
  }
  const teaser = newsTeaser();
  if (teaser && !snap.account.offerBackup) out.push(teaser);
  return out;
}

function playRunning(snap: Snapshot): HTMLElement[] {
  return [eyebrow(snap), ...heading(t("running_title"), t("running_text")), card("card-accent", h("h3", { class: "card-title" }, t("running_relay_title")), h("p", { class: "card-text" }, t("running_relay_text")))];
}

function renderPlay(): void {
  const snap = state.snap;
  const container = $("#view-play");
  if (!snap) {
    renderRegion(container, "loading", () => [h("div", { class: "play-content" }, h("div", { class: "card-row" }, h("div", { class: "spinner" }), t("loading")))]);
    return;
  }
  const sig = JSON.stringify([
    state.lang,
    snap.phase,
    snap.busy,
    snap.server,
    snap.connect,
    snap.account,
    snap.game,
    snap.runtime,
    snap.settings,
    snap.status?.registration ?? null,
    snap.status?.name ?? null,
    snap.status?.gameData ?? null,
    snap.lastError?.code === "key_rejected",
    state.keyFormOpen,
    state.pathFormOpen,
    state.news[0]?.title ?? null,
    snap.phase === "installing" ? state.task?.kind ?? snap.task?.kind : null,
  ]);
  renderRegion(container, sig, () => {
    let parts: HTMLElement[];
    switch (snap.phase) {
      case "loading":
        parts = [h("div", { class: "card-row" }, h("div", { class: "spinner" }), t("loading"))];
        break;
      case "join":
        parts = playJoin(snap);
        break;
      case "connect":
        parts = playConnect(snap);
        break;
      case "register":
        parts = playRegister(snap);
        break;
      case "locate":
        parts = playLocate(snap);
        break;
      case "runtime":
        parts = playRuntime(snap);
        break;
      case "installing":
        parts = playInstalling(snap);
        break;
      case "ready":
        parts = playReady(snap);
        break;
      case "running":
        parts = playRunning(snap);
        break;
    }
    const vc = vcWarning();
    return [h("div", { class: "play-content" }, ...parts, vc)];
  });
}

// ------------------------------------------------------------------ action bar

const progressEls = (() => {
  const title = h("span", { class: "progress-title" });
  const percent = h("span", { class: "progress-percent" });
  const fill = h("div", { class: "bar-fill" });
  const bar = h("div", { class: "bar", role: "progressbar", "aria-valuemin": 0, "aria-valuemax": 100 }, fill);
  const meta = h("div", { class: "progress-meta" });
  const file = h("div", { class: "progress-file" });
  // Not a live region: the numbers change several times a second. The progressbar role gives the
  // value on request, and #action-live announces the milestones.
  const root = h("div", { class: "progress", "aria-live": "off" }, h("div", { class: "progress-head" }, title, percent), bar, meta, file);
  return { root, title, percent, fill, bar, meta, file };
})();

function renderProgress(task: TaskProgress): void {
  const v = taskView(task, state.lang);
  const p = progressEls;
  p.title.textContent = t(v.titleKey);
  p.percent.textContent = formatPercentText(v.percent, state.lang);
  p.fill.style.width = `${v.percent}%`;
  p.bar.setAttribute("aria-valuenow", String(Math.round(v.percent)));
  p.bar.setAttribute("aria-label", t(v.titleKey));
  const bits: (string | null)[] = [v.bytesText, v.speedText, v.etaText ? t("task_eta", { time: v.etaText }) : null, t("task_files", { done: task.filesDone, total: task.filesTotal })];
  p.meta.replaceChildren(...bits.filter((b): b is string => b !== null).map((b) => h("span", {}, b)));
  p.file.textContent = task.currentFile ? t("task_current", { file: task.currentFile }) : "";
}

function renderSteps(snap: Snapshot): HTMLElement {
  const current = stepIndex(snap.phase);
  const items: HTMLElement[] = [];
  STEP_KEYS.forEach((k, i) => {
    const cls = i < current || snap.phase === "running" ? "done" : i === current ? "current" : "";
    if (i > 0) items.push(h("li", { class: `step-line ${i <= current ? "done" : ""}`, "aria-hidden": "true" }));
    items.push(h("li", { class: `step ${cls}`, "aria-current": i === current ? "step" : undefined }, h("span", { class: "step-dot" }, cls === "done" ? icon("check") : String(i + 1)), h("span", { class: "step-label" }, t(k))));
  });
  return h("ol", { class: "steps", "aria-label": t("steps_label") }, ...items);
}

function actionHint(snap: Snapshot): string {
  switch (snap.phase) {
    case "join":
      return t("hint_join");
    case "connect":
      return snap.connect.checking ? t("hint_connecting") : snap.connect.problem === "cert_mismatch" ? t("hint_cert") : t("hint_connect");
    case "register":
      return t("hint_register");
    case "locate":
      if (snap.revived) return snap.revived.problem === "not_set" ? t("hint_revived_download") : t("hint_revived_check");
      return snap.game.problem === "unverified" ? t("hint_verify") : t("hint_locate");
    case "runtime":
      return snap.runtime.state === "unavailable" ? t("hint_runtime_none") : snap.runtime.state === "untrusted" ? t("hint_trust") : t("hint_runtime");
    case "ready":
      return t("hint_ready");
    case "running":
      return t("hint_running");
    default:
      return "";
  }
}

const primary = $("#primary") as HTMLButtonElement;

function renderActionBar(): void {
  const snap = state.snap;
  const info = $("#action-info");
  if (!snap) {
    primary.disabled = true;
    primary.textContent = t("btn_wait");
    return;
  }
  const b = primaryButton(snap, formState());
  primary.textContent = t(b.labelKey);
  primary.disabled = b.disabled;
  primary.classList.toggle("is-running", b.action === "none" && snap.phase === "running");
  primary.classList.toggle("is-pause", b.action === "cancel");
  primary.classList.toggle("is-ready", b.action === "play" && !b.disabled && !reducedMotion.matches);
  primary.setAttribute("data-action", b.action);

  const task = snap.phase === "installing" ? state.task ?? snap.task : null;
  if (task) {
    if (!info.contains(progressEls.root)) info.replaceChildren(progressEls.root);
    renderProgress(task);
    forceRender(info);
    const v = taskView(task, state.lang);
    announce(`${t(v.titleKey)}: ${Math.floor(v.percent / 10) * 10} %`);
  } else {
    renderRegion(info, JSON.stringify([state.lang, snap.phase, snap.connect.checking, snap.connect.problem, snap.game.problem, snap.runtime.state, snap.revived?.problem ?? null]), () => [renderSteps(snap), h("div", { class: "action-hint" }, actionHint(snap))]);
    announce(actionHint(snap));
  }
}

// The action bar's screen reader announcements: what to do next, and task milestones (every
// 10 %). Nothing else in the bar is a live region.
function announce(text: string): void {
  const live = $("#action-live");
  if (live.textContent !== text) live.textContent = text;
}

async function onPrimary(): Promise<void> {
  const snap = state.snap;
  if (!snap) return;
  const b = primaryButton(snap, formState());
  if (b.disabled || b.action === "none") return;
  state.localError = null;
  let result: { ok: boolean; error?: LauncherError } | undefined;
  switch (b.action) {
    case "join": {
      const text = state.inviteText.trim();
      // A changed certificate gets the warning first, never a direct join.
      const m = await inviteModal(text);
      if (m?.kind === "cert_changed" || m?.kind === "server_moved") {
        showModal(m);
        return;
      }
      result = await api.submitInvite(text);
      if (result.ok) clearInviteInput();
      else if (result.error?.code === "cert_changed") {
        const again = await inviteModal(text);
        if (again) showModal(again);
        return;
      }
      break;
    }
    case "retry":
      result = await api.retryConnect();
      break;
    case "new_invite":
      showModal({ kind: "leave" });
      return;
    case "register":
      result = await api.register(state.username.trim());
      if (result.ok) {
        state.username = "";
        usernameInput.value = "";
        updateUsernameFeedback();
      }
      break;
    case "locate":
      result = await api.chooseGameFolder();
      break;
    case "verify":
      result = await api.verifyGame();
      break;
    case "revived_install":
      result = await api.revivedInstall("install");
      break;
    case "trust":
      showModal({ kind: "trust_runtime" });
      return;
    case "install_runtime":
      result = await api.installRuntime();
      break;
    case "play":
      result = await api.play();
      break;
    case "cancel":
      await api.cancelTask();
      return;
  }
  if (result && !result.ok && result.error && result.error.code !== "cancelled") {
    const latest = await api.getSnapshot();
    if (!latest.lastError) state.localError = result.error;
    onSnapshot(latest);
  }
}

primary.addEventListener("click", () => void onPrimary());

// ------------------------------------------------------------------ server panel

function instanceTitle(i: StatusInstance): string {
  switch (i.kind) {
    case "city":
      return t("inst_title_city");
    case "dojo":
      return t("inst_title_dojo");
    case "tutorial":
      return t("inst_title_tutorial");
    case "hunt":
      return t("inst_title_hunt", { behemoth: i.behemoth ? i.title || i.behemoth : i.title || t("kind_hunt") });
  }
}

const KIND_ICON: Record<InstanceKind, IconName> = { city: "city", hunt: "hunt", dojo: "dojo", tutorial: "tutorial" };

function instanceCard(i: StatusInstance): HTMLElement {
  const pct = i.maxPlayers > 0 ? Math.min(100, (i.players / i.maxPlayers) * 100) : 0;
  const fill = h("span", {});
  fill.style.width = `${pct}%`;
  return h(
    "div",
    { class: `instance k-${i.kind}` },
    h("div", { class: "instance-icon" }, icon(KIND_ICON[i.kind])),
    h("div", { class: "instance-title", title: instanceTitle(i) }, instanceTitle(i)),
    h(
      "div",
      { class: "instance-meta" },
      h("span", {}, i.maxPlayers > 0 ? t("inst_players", { n: i.players, max: i.maxPlayers }) : t("inst_players_nomax", { n: i.players })),
      h("span", {}, t("inst_running", { time: formatRunningTime(i.startedAt, Date.now(), state.lang) })),
    ),
    h("div", { class: "instance-bar", "aria-hidden": "true" }, fill),
  );
}

function playerList(status: ServerStatus, max: number): HTMLElement {
  const byId = new Map(status.instances.map((i) => [i.id, i]));
  const items = status.players.slice(0, max).map((p) => {
    const inst = p.instance ? byId.get(p.instance) : undefined;
    const where = inst && inst.kind === "hunt" ? `${t(whereKey(p.where))} · ${instanceTitle(inst)}` : t(whereKey(p.where));
    return h(
      "li",
      { class: `player w-${p.where}` },
      h("span", { class: "player-avatar", "aria-hidden": "true" }, Array.from(p.name)[0]?.toUpperCase() ?? "?"),
      h("span", { class: "player-text" }, h("span", { class: `player-name${streamer() ? " secret-blur" : ""}` }, streamer() ? filler(p.name) : p.name), h("span", { class: "player-where" }, where)),
    );
  });
  const list = h("ul", { class: "players" }, ...items);
  if (status.players.length > max) list.appendChild(h("li", { class: "player-where" }, t("sp_more", { n: status.players.length - max })));
  return list;
}

function renderPanel(): void {
  const panel = $("#server-panel");
  const snap = state.snap;
  const minute = Math.floor(Date.now() / 30000);
  const sig = JSON.stringify([state.lang, snap?.server ?? null, snap?.status ?? null, snap?.statusUnsupported, snap?.phase === "connect", snap?.connect.problem, snap?.connect.checking, snap?.settings.streamerMode, minute]);
  renderRegion(panel, sig, () => {
    if (!snap || !snap.server) {
      return [h("div", { class: "sp-head" }, h("span", { class: "sp-kicker" }, t("sp_title")), h("span", { class: "sp-name" }, t("rail_not_joined"))), h("p", { class: "sp-empty" }, t("sp_nojoin")), panelFoot(null)];
    }
    const status = snap.status;
    const reachable = snap.phase !== "connect";
    const online = reachable && (status ? status.online : true);
    const head = h(
      "div",
      { class: "sp-head" },
      h("span", { class: "sp-kicker" }, t("sp_title")),
      h("span", { class: "sp-name" }, serverName()),
      h("div", { class: "sp-state" }, h("span", { class: `dot ${online ? "online" : "offline"}`, "aria-hidden": "true" }), online ? t("sp_online") : t("sp_offline"), modeBadge(snap.server.mode)),
    );
    if (snap.phase === "connect" && !snap.connect.checking && snap.connect.problem) {
      const text = snap.connect.problem === "cert_mismatch" ? t("connect_cert_subtitle") : t("sp_no_answer");
      return [head, h("p", { class: `sp-empty${snap.connect.problem === "cert_mismatch" ? " sp-problem" : ""}` }, text), panelFoot(null)];
    }
    if (!status) return [head, h("p", { class: "sp-empty" }, reachable && snap.statusUnsupported ? t("sp_unsupported") : t("sp_waiting")), panelFoot(null)];
    if (status.limited) {
      return [head, h("div", { class: "sp-section" }, h("div", { class: "sp-section-title" }, t("server_players"), icon("people")), h("p", { class: "sp-empty" }, t("sp_signin"))), panelFoot(status)];
    }
    const instances = sortInstances(status.instances);
    return [
      head,
      h(
        "div",
        { class: "sp-section" },
        h("div", { class: "sp-section-title" }, t("server_players"), icon("people")),
        h("div", { class: "sp-count" }, h("span", { class: "sp-count-num" }, String(status.playersOnline)), h("span", { class: "sp-count-label" }, t(status.playersOnline === 1 ? "sp_players_one_label" : "sp_players_other_label"))),
        status.players.length > 0 ? playerList(status, 8) : h("p", { class: "sp-empty" }, t("sp_nobody")),
      ),
      h(
        "div",
        { class: "sp-section" },
        h("div", { class: "sp-section-title" }, t("sp_worlds")),
        instances.length > 0 ? h("div", { class: "instances" }, ...instances.slice(0, 6).map(instanceCard)) : h("p", { class: "sp-empty" }, t("sp_no_worlds")),
        instances.length > 6 ? linkButton(t("sp_all_worlds", { n: instances.length }), () => setView("server"), { fk: "all-worlds" }) : null,
      ),
      panelFoot(status),
    ];
  });
}

function panelFoot(status: ServerStatus | null): HTMLElement {
  return h(
    "div",
    { class: "sp-foot" },
    status
      ? h("div", { class: "sp-foot-row" }, h("span", {}, t("sp_version", { version: [status.version, status.commit].filter(Boolean).join(" · ") || "–" })), linkButton(t("sp_source"), () => open("server_source"), { icon: "external", fk: "sp-source" }))
      : null,
    h("div", { class: "sp-foot-row" }, h("span", {}, t("about_version", { v: state.snap?.app.version ?? "" })), linkButton(t("about_source"), () => open("project_source"), { icon: "external", fk: "sp-launcher-source" })),
  );
}

// ------------------------------------------------------------------ news page

function renderNews(): void {
  const container = $("#view-news");
  const snap = state.snap;
  const sig = JSON.stringify([state.lang, state.news, snap?.server?.name ?? null, snap?.status?.name ?? null]);
  renderRegion(container, sig, () => {
    const title = snap?.server ? t("news_title", { name: serverName() }) : t("news_title_generic");
    const body: HTMLElement[] = [];
    if (!snap?.server) body.push(card("", h("p", { class: "card-text" }, t("news_offline"))));
    else if (state.news.length === 0) body.push(card("", h("p", { class: "card-text" }, t("news_empty"))));
    else {
      for (const n of state.news) {
        body.push(h("article", { class: "card news-item" }, n.date ? h("time", { class: "news-date", datetime: n.date }, formatDate(n.date, state.lang)) : null, h("h2", { class: "news-title" }, n.title), h("p", { class: "news-body" }, n.body)));
      }
    }
    return [h("div", { class: "page" }, h("h1", { class: "page-title", id: "news-title" }, title), ...body)];
  });
}

// ------------------------------------------------------------------ server page

function renderServer(): void {
  const container = $("#view-server");
  const snap = state.snap;
  const minute = Math.floor(Date.now() / 30000);
  const sig = JSON.stringify([state.lang, snap?.server ?? null, snap?.savedServers ?? [], snap?.status ?? null, snap?.runtime ?? null, snap?.phase, snap?.connect.problem, snap?.busy, !!snap?.task, snap?.running.game, snap?.settings.streamerMode, minute]);
  renderRegion(container, sig, () => {
    if (!snap?.server) return [h("div", { class: "page" }, h("h1", { class: "page-title", id: "server-title" }, t("nav_server")), card("", h("p", { class: "card-text" }, t("sp_nojoin"))))];
    const sv = snap.server;
    const status = snap.status;
    const rows: [string, Node | string][] = [
      [t("server_address"), secret(`${sv.host}:${sv.port}`, "server-address", "mono")],
      [t("server_connection"), sv.mode === "public" ? t("server_connection_public") : t("server_connection_private")],
    ];
    if (sv.fingerprint) rows.push([t("server_fingerprint"), secret(groupedFingerprint(sv.fingerprint), "server-fp", "fingerprint")]);
    if (status) {
      rows.push([t("server_registration"), status.registration ? tk(`reg_mode_${status.registration}`) : "–"]);
      rows.push([t("server_version"), [status.version, status.commit].filter(Boolean).join(" · ") || "–"]);
      rows.push([t("server_uptime"), formatDuration(status.uptimeSeconds, state.lang)]);
      if (status.game) rows.push([t("server_game"), `${status.game.build}${status.game.changelist ? ` (CL ${status.game.changelist})` : ""}`]);
      rows.push([t("server_gamedata"), tk(`gamedata_kind_${status.gameData}`)]);
      rows.push([t("server_chat"), t(status.chat ? "server_chat_on" : "server_chat_off")]);
    }
    rows.push([t("server_runtime"), snap.runtime.version ? `${snap.runtime.version} · ${tk(`runtime_state_${snap.runtime.state}`)}` : tk(`runtime_state_${snap.runtime.state}`)]);
    const kv = h("dl", { class: "kv" });
    for (const [k, v] of rows) kv.append(h("dt", {}, k), h("dd", {}, v));
    const parts: HTMLElement[] = [
      h("h1", { class: "page-title", id: "server-title" }, serverName()),
      h("p", { class: "page-sub" }, t("server_auto")),
      card("", kv, h("div", { class: "card-row" }, button(t("sp_refresh"), () => void api.refreshStatus(), { icon: "refresh", fk: "srv-refresh" }), status?.sourceUrl ? linkButton(t("sp_source"), () => open("server_source"), { icon: "external", fk: "srv-source" }) : null)),
    ];
    if (status?.limited) parts.push(card("", h("h2", { class: "card-title" }, t("server_players")), h("p", { class: "card-text" }, t("sp_signin"))));
    else if (status) {
      const instances = sortInstances(status.instances);
      parts.push(
        h(
          "div",
          { class: "grid-2" },
          card("", h("h2", { class: "card-title" }, t("server_players_count", { n: status.playersOnline })), status.players.length ? playerList(status, 100) : h("p", { class: "card-text" }, t("sp_nobody"))),
          card("", h("h2", { class: "card-title" }, t("server_worlds")), instances.length ? h("div", { class: "instances" }, ...instances.map(instanceCard)) : h("p", { class: "card-text" }, t("sp_no_worlds"))),
        ),
      );
    }
    const saved = savedServersCard(snap);
    if (saved) parts.push(saved);
    parts.push(h("div", { class: "card-row" }, leaveLink(snap)));
    return [h("div", { class: "page page-server" }, ...parts)];
  });
}

// ------------------------------------------------------------------ host page

function hostRefresh(): Promise<void> {
  return api.hostGet().then((k) => {
    state.host = k;
    renderHost();
  });
}

function hostAct(action: HostAction): void {
  void api.hostAction(action).then(async (r) => {
    await report(r);
    await hostRefresh();
  });
}

function hostOpen(folder: HostFolder): void {
  void api.hostOpen(folder).then(report);
}

function cmd(text: string): HTMLElement {
  return h("div", { class: "cmd" }, icon("terminal"), h("span", {}, text));
}

function hostStep(n: number, title: string, stepState: StepState, chipText: string, ...body: (HTMLElement | null)[]): HTMLElement {
  return card(`host-step${stepState === "done" ? " is-done" : ""}`, h("div", { class: "host-step-head" }, h("span", { class: "host-step-num" }, stepState === "done" ? icon("check") : String(n)), h("h2", { class: "host-step-title" }, title), chip(stepState, chipText)), ...body);
}

// The Host page's tabs: the server (start, stop, setup, invites), its game data, the builds that need
// the game (SDK, runtime, CatalogExporter) and what this PC needs installed.
type HostTab = "server" | "gamedata" | "build" | "requirements";
// In the order a new host goes through them: what to install, the builds, the game data, then the server.
const HOST_TABS: HostTab[] = ["requirements", "build", "gamedata", "server"];
const HOST_TAB_ICONS: Record<HostTab, IconName> = { server: "server", gamedata: "database", build: "hammer", requirements: "list" };

// The server setup form and the invite note keep what was typed across renders.
const setupNameInput = h("input", { class: "input", id: "host-setup-name", type: "text", maxlength: 60, spellcheck: "false", autocomplete: "off", "data-fk": "host-setup-name" });
const setupHostInput = h("input", { class: "input", id: "host-setup-host", type: "text", maxlength: 253, spellcheck: "false", autocomplete: "off", placeholder: "203.0.113.7", "data-fk": "host-setup-host" });
// Text fields (a number field has no caret position for the page to keep across renders).
const multXpInput = h("input", { class: "input input-mult", id: "host-mult-xp", type: "text", maxlength: 6, inputmode: "decimal", spellcheck: "false", autocomplete: "off", "data-fk": "host-mult-xp" });
const multRewardsInput = h("input", { class: "input input-mult", id: "host-mult-rewards", type: "text", maxlength: 6, inputmode: "decimal", spellcheck: "false", autocomplete: "off", "data-fk": "host-mult-rewards" });
for (const input of [multXpInput, multRewardsInput]) input.addEventListener("input", () => renderHost());
const inviteForInput = h("input", { class: "input", id: "host-invite-for", type: "text", maxlength: 40, spellcheck: "false", autocomplete: "off", "data-fk": "host-invite-for" });
inviteForInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") void createInvite();
});

async function createInvite(): Promise<void> {
  const r = await api.hostCreateInvite(inviteForInput.value.trim() || null);
  await report(r);
  if (r.ok) inviteForInput.value = "";
}

async function copy(text: string, key: string): Promise<void> {
  if (await api.copyText(text)) {
    state.copied = key;
    renderHost();
    window.setTimeout(() => {
      if (state.copied === key) {
        state.copied = null;
        renderHost();
      }
    }, 2000);
  }
}

// What each requirement links to, and how many install steps its guide has (req_<id>_s1..sN).
const REQUIREMENTS: Record<RequirementId, { download?: ExternalTarget; page?: ExternalTarget; steps: number }> = {
  dauntless: { page: "installing_guide", steps: 2 },
  git: { download: "git", steps: 3 },
  node: { page: "nodejs", steps: 3 },
  mongodb: { page: "mongodb", steps: 5 },
  tailscale: { page: "tailscale_download", steps: 3 },
  visual_studio: { download: "visual_studio_installer", page: "visual_studio", steps: 4 },
  rust: { download: "rust_installer", page: "rust", steps: 4 },
  dumper7: { download: "dumper7_zip", page: "dumper7", steps: 3 },
  vc_redist: { download: "vc_redist", steps: 2 },
};
const REQUIREMENT_GROUPS: { key: StringKey; ids: RequirementId[] }[] = [
  { key: "hreq_group_host", ids: ["dauntless", "git", "node", "mongodb", "tailscale"] },
  { key: "hreq_group_build", ids: ["visual_studio", "rust", "dumper7"] },
  { key: "hreq_group_play", ids: ["vc_redist"] },
];
const MONGODB_SERVICE_FIX = "Stop-Service MongoDB; Set-Service MongoDB -StartupType Disabled";

function requirementCard(r: Requirement): HTMLElement {
  const info = REQUIREMENTS[r.id];
  // The steps are open for whatever is missing, until the host closes them (a problem has its own text).
  const open = state.reqOpen.has(r.id) || (r.state === "missing" && !state.reqClosed.has(r.id));
  const stateChip = chip(r.state === "ok" ? "ok" : r.state === "problem" ? "warn" : "todo", `${tk(`hreq_state_${r.state}`)}${r.detail && r.state === "ok" ? ` · ${r.detail}` : ""}`);
  const problem =
    r.problem === "service_running"
      ? h("div", { class: "stack" }, h("p", { class: "card-text" }, t("req_mongodb_service_running")), h("div", { class: "card-row" }, cmd(MONGODB_SERVICE_FIX), button(state.copied === "mongo-fix" ? t("hreq_copied") : t("hreq_copy"), () => void copy(MONGODB_SERVICE_FIX, "mongo-fix"), { icon: state.copied === "mongo-fix" ? "check" : "copy", fk: "req-mongo-copy" })))
      : r.problem === "too_old"
        ? h("p", { class: "card-text" }, t("req_node_too_old", { version: r.detail ?? "?" }))
        : r.problem === "no_cpp"
          ? h("p", { class: "card-text" }, t("req_visual_studio_no_cpp", { version: r.detail ?? "Visual Studio" }))
          : null;
  const toggle = linkButton(open ? t("hreq_hide") : t("hreq_how"), () => {
    if (open) {
      state.reqOpen.delete(r.id);
      state.reqClosed.add(r.id);
    } else {
      state.reqOpen.add(r.id);
      state.reqClosed.delete(r.id);
    }
    renderHost();
  }, { icon: "info", fk: `req-how-${r.id}` });
  const steps = open ? h("ol", { class: "steps-list" }, ...Array.from({ length: info.steps }, (_, i) => h("li", {}, h("div", {}, tk(`req_${r.id}_s${i + 1}`))))) : null;
  return card(
    `host-req${r.state === "ok" ? " is-done" : ""}`,
    h("div", { class: "host-step-head" }, h("span", { class: "host-step-num" }, r.state === "ok" ? icon("check") : icon(r.state === "problem" ? "warning" : "download")), h("h3", { class: "host-step-title" }, tk(`req_${r.id}_name`)), stateChip),
    h("p", { class: "card-text" }, tk(`req_${r.id}_why`)),
    problem,
    h(
      "div",
      { class: "card-row" },
      info.download ? button(t("hreq_download"), () => open_(info.download!), { cls: r.state === "ok" ? "" : "btn-primary", icon: "download", fk: `req-dl-${r.id}` }) : null,
      info.page ? (info.download ? linkButton(t("hreq_open_page"), () => open_(info.page!), { icon: "external", fk: `req-page-${r.id}` }) : button(t("hreq_open_page"), () => open_(info.page!), { cls: r.state === "ok" ? "" : "btn-primary", icon: "external", fk: `req-page-${r.id}` })) : null,
      toggle,
    ),
    steps,
  );
}

function open_(target: ExternalTarget): void {
  open(target);
}

function hostTabs(): HTMLElement {
  const tabs = h("div", { class: "settings-tabs host-tabs", role: "tablist", "aria-label": t("host_title") });
  for (const tab of HOST_TABS) {
    const on = state.hostTab === tab;
    const b = h("button", { type: "button", class: "settings-tab", role: "tab", id: `host-tab-${tab}`, "aria-selected": on ? "true" : "false", "aria-controls": "host-panel", tabindex: on ? "0" : "-1", "data-fk": `host-tab-${tab}`, title: tk(`host_tab_${tab}`) }, icon(HOST_TAB_ICONS[tab]), h("span", { class: "settings-tab-label" }, tk(`host_tab_${tab}`)));
    b.addEventListener("click", () => setHostTab(tab));
    b.addEventListener("keydown", (e) => {
      const i = HOST_TABS.indexOf(tab);
      const next = e.key === "ArrowRight" ? HOST_TABS[(i + 1) % HOST_TABS.length] : e.key === "ArrowLeft" ? HOST_TABS[(i + HOST_TABS.length - 1) % HOST_TABS.length] : null;
      if (!next) return;
      e.preventDefault();
      setHostTab(next);
      document.getElementById(`host-tab-${next}`)?.focus();
    });
    tabs.appendChild(b);
  }
  return tabs;
}

function setHostTab(tab: HostTab): void {
  state.hostTab = tab;
  // The requirements are looked at again when their tab opens.
  if (tab === "requirements") void hostRefresh();
  renderHost();
}

function workspaceCard(k: HostKit, steps: ReturnType<typeof hostSteps>): HTMLElement {
  return hostStep(
    1,
    t("host_ws_title"),
    steps.workspace,
    steps.workspace === "done" ? t("host_ws_ready") : k.workspaceOk ? t("host_ws_unpatched") : t("host_ws_missing"),
    h("p", { class: "card-text" }, t("host_ws_text")),
    k.workspace ? h("p", { class: "small-print selectable" }, secret(k.workspace, "host-ws")) : null,
    // Without one, the launcher downloads it beside the game (it does on its own when Host tools turn on).
    !k.workspaceOk ? h("p", { class: "small-print" }, k.projectTarget ? t("host_ws_download_to", { folder: k.projectTarget }) : t("host_ws_download_no_game")) : null,
    h(
      "div",
      { class: "card-row" },
      !k.workspaceOk ? button(t("host_ws_download"), () => void api.hostDownloadProject().then(report).then(hostRefresh), { cls: "btn-primary", icon: "download", fk: "host-ws-download", disabled: k.job?.state === "running" || !k.projectTarget }) : null,
      button(t("host_ws_choose"), () => void api.hostChooseWorkspace().then(report).then(hostRefresh), { icon: "folder", fk: "host-ws" }),
      k.workspace ? linkButton(t("host_open"), () => hostOpen("workspace"), { icon: "folder", fk: "host-ws-open" }) : null,
    ),
  );
}

function checkRow(ok: StepState, text: string, fix: (() => void) | null): HTMLElement {
  return h("div", { class: "host-check" }, chip(ok, ""), h("span", { class: "host-check-text" }, text), fix && ok !== "done" ? linkButton(t("hsrv_fix"), fix, { fk: `host-fix-${text.length}` }) : null);
}

function serverTab(snap: Snapshot, k: HostKit, steps: ReturnType<typeof hostSteps>): HTMLElement[] {
  const v = state.hostServer;
  const st = v?.state ?? "stopped";
  const jobRunning = k.job?.state === "running";
  const configured = k.serverConfig.configured && k.upstream.patched;
  const gdState: StepState = steps.gameData === "done" ? "done" : steps.gameData === "warn" ? "warn" : "todo";
  const active = st === "starting" || st === "running" || st === "stopping";
  const canStart = k.workspaceOk && configured && gdState !== "todo" && !jobRunning && (st === "stopped" || st === "failed");
  const stateClass = st === "running" ? "ok" : st === "failed" ? "danger" : st === "external" ? "warn" : active ? "busy" : "idle";
  const text = st === "failed" ? (v?.message ? t("hsrv_text_failed", { reason: v.message }) : t("hsrv_text_failed_noreason")) : tk(`hsrv_text_${st}`);
  const parts: HTMLElement[] = [];
  if (!k.workspaceOk) parts.push(workspaceCard(k, steps));

  parts.push(
    card(
      `host-server host-server-${stateClass}`,
      h("h2", { class: "card-title" }, t("hsrv_title")),
      h("div", { class: "host-server-head" }, h("span", { class: `host-server-dot ${stateClass}` }), h("span", { class: "host-server-state" }, tk(`hsrv_state_${st}`)), st === "starting" || st === "stopping" ? h("div", { class: "spinner" }) : null),
      h("p", { class: "card-text" }, text),
      st === "running" && k.serverConfig.host ? h("p", { class: "small-print selectable" }, t("hsrv_address", { address: "" }), secret(k.serverConfig.host, "host-address")) : null,
      h(
        "div",
        { class: "card-row" },
        active
          ? button(t("hsrv_stop"), () => void api.hostServerStop().then(report), { cls: "btn-danger", icon: "stop", fk: "host-stop", disabled: st === "stopping" })
          : button(t("hsrv_start"), () => void api.hostServerStart().then(report), { cls: "btn-primary btn-big", icon: "play", fk: "host-start", disabled: !canStart }),
      ),
      h(
        "div",
        { class: "host-checks" },
        h("div", { class: "field-label" }, t("hsrv_checks")),
        checkRow(k.workspaceOk ? "done" : "todo", t("hsrv_check_workspace"), null),
        checkRow(configured ? "done" : "todo", t("hsrv_check_setup"), () => document.getElementById("host-setup-name")?.focus()),
        checkRow(gdState, gdState === "warn" ? t("hsrv_check_gamedata_example") : t("hsrv_check_gamedata"), () => setHostTab("gamedata")),
        checkRow(k.builds.published ? "done" : "warn", t("hsrv_check_runtime"), () => setHostTab("build")),
      ),
    ),
  );

  // Prepare / update (npm run setup)
  if (!setupNameInput.value && k.serverConfig.name) setupNameInput.value = k.serverConfig.name;
  const mode = state.setupMode ?? k.serverConfig.mode ?? "private";
  const tailscaleMissing = mode === "private" && k.requirements.some((r) => r.id === "tailscale" && r.state !== "ok");
  const modeOption = (m: "private" | "public") => {
    const input = h("input", { type: "radio", name: "host-setup-mode", value: m, id: `host-mode-${m}`, "data-fk": `host-mode-${m}` });
    input.checked = mode === m;
    input.addEventListener("change", () => {
      state.setupMode = m;
      renderHost();
    });
    return h("label", { class: "radio-row", for: `host-mode-${m}` }, input, h("span", {}, tk(`hsrv_mode_${m}`)));
  };
  const prepareAt = parts.length;
  parts.push(
    card(
      "",
      h("div", { class: "host-step-head" }, h("h2", { class: "host-step-title" }, t("hsrv_setup_title")), chip(configured ? "done" : "todo", configured ? t("hsrv_setup_done", { name: k.serverConfig.name ?? "", mode: tk(`hsrv_mode_short_${k.serverConfig.mode ?? "private"}`) }) : t("host_ws_unpatched"))),
      h("p", { class: "card-text" }, t("hsrv_setup_text")),
      h("div", { class: "field" }, h("label", { class: "field-label", for: "host-setup-name" }, t("hsrv_setup_name")), setupNameInput),
      h("div", { class: "field", role: "radiogroup", "aria-label": t("hsrv_setup_mode") }, h("div", { class: "field-label" }, t("hsrv_setup_mode")), modeOption("private"), modeOption("public")),
      mode === "public" ? h("div", { class: "field" }, h("label", { class: "field-label", for: "host-setup-host" }, t("hsrv_setup_host")), setupHostInput) : null,
      snap.game.root ? h("p", { class: "small-print selectable" }, t("hsrv_setup_game", { folder: "" }), secret(snap.game.root, "host-setup-game")) : h("p", { class: "small-print warn-text" }, t("hsrv_setup_no_game")),
      tailscaleMissing ? h("p", { class: "small-print warn-text" }, t("hsrv_setup_tailscale")) : null,
      h(
        "div",
        { class: "card-row" },
        button(configured ? t("hsrv_setup_update") : t("hsrv_setup_run"), () => void api.hostSetup({ name: setupNameInput.value, mode, publicHost: mode === "public" ? setupHostInput.value : null }).then(report).then(hostRefresh), {
          cls: configured ? "" : "btn-primary",
          icon: configured ? "refresh" : "hammer",
          fk: "host-setup",
          disabled: !k.workspaceOk || jobRunning || active || st === "external" || !setupNameInput.value.trim(),
        }),
      ),
    ),
  );

  // Invites
  const canInvite = st === "running" || st === "external";
  const invite = v?.invite ?? null;
  parts.push(
    card(
      "",
      h("h2", { class: "card-title" }, t("hsrv_invite_title")),
      h("p", { class: "card-text" }, t("hsrv_invite_text")),
      h("div", { class: "field" }, h("label", { class: "field-label", for: "host-invite-for" }, t("hsrv_invite_for")), inviteForInput),
      h("div", { class: "card-row" }, button(t("hsrv_invite_create"), () => void createInvite(), { cls: "btn-primary", icon: "link", fk: "host-invite", disabled: !canInvite }), canInvite ? null : h("span", { class: "small-print" }, t("hsrv_invite_need_running"))),
      invite
        ? h(
            "div",
            { class: "host-invite" },
            h("div", { class: "field-label" }, t("hsrv_invite_ready", { for: invite.for ? ` · ${invite.for}` : "" })),
            h("div", { class: "host-invite-link selectable" }, secret(invite.link, "host-invite-link")),
            h("div", { class: "card-row" }, button(state.copied === "invite" ? t("hsrv_invite_copied") : t("hsrv_invite_copy"), () => void copy(invite.link, "invite"), { icon: state.copied === "invite" ? "check" : "copy", fk: "host-invite-copy" })),
            k.serverConfig.mode === "private" ? h("p", { class: "small-print" }, t("hsrv_invite_tailscale")) : null,
          )
        : null,
    ),
  );
  // Then the test multipliers, updating, and the server's settings (which change least).
  const prepareCard = parts.splice(prepareAt, 1)[0];
  parts.push(multipliersCard(k, st, jobRunning), updateCard(k, st, jobRunning), prepareCard);

  // The output of the server and of the commands started here.
  const consoleBox = h("div", { class: "host-console-wrap", id: "host-console" });
  parts.push(
    card(
      "",
      h("div", { class: "host-step-head" }, h("h2", { class: "host-step-title" }, t("hsrv_console")), linkButton(state.consoleOpen ? t("hsrv_console_hide") : t("hsrv_console_show"), () => ((state.consoleOpen = !state.consoleOpen), renderHost()), { icon: "terminal", fk: "host-console-toggle" }), linkButton(t("host_open_logs"), () => hostOpen("host_logs"), { icon: "folder", fk: "host-console-logs" })),
      state.consoleOpen ? consoleBox : null,
    ),
  );
  return parts;
}

// A typed multiplier: a number from 1 to 100, or null while it is not one.
function multiplierValue(input: HTMLInputElement): number | null {
  const n = Number(input.value.trim().replace(",", ".").replace(/^x/i, ""));
  return input.value.trim() !== "" && Number.isFinite(n) && n >= 1 && n <= MAX_MULTIPLIER ? Math.round(n * 100) / 100 : null;
}

// The test multipliers (ReforgedHost's npm run multipliers), private servers only.
function multipliersCard(k: HostKit, st: HostServerView["state"], jobRunning: boolean): HTMLElement {
  const current = k.serverConfig.multipliers;
  const isPrivate = k.serverConfig.mode === "private";
  // The fields start at what is set (1 when off) and keep what was typed afterwards.
  for (const [input, value] of [[multXpInput, current?.xp ?? 1], [multRewardsInput, current?.rewards ?? 1]] as const) {
    if (input.value === "" && document.activeElement !== input) input.value = String(value);
  }
  const xp = multiplierValue(multXpInput);
  const rewards = multiplierValue(multRewardsInput);
  multXpInput.setAttribute("aria-invalid", xp === null ? "true" : "false");
  multRewardsInput.setAttribute("aria-invalid", rewards === null ? "true" : "false");
  const blocked = !k.serverConfig.configured || !isPrivate || jobRunning || st === "external" || st === "starting" || st === "stopping";
  const same = current !== null && xp === current.xp && rewards === current.rewards;
  const field = (input: HTMLInputElement, label: string) => h("label", { class: "field mult-field", for: input.id }, h("span", { class: "field-label" }, label), h("span", { class: "mult-input" }, h("span", { class: "mult-x" }, "×"), input));
  return card(
    "",
    h("div", { class: "host-step-head" }, h("h2", { class: "host-step-title" }, t("hmult_title")), current ? chip("warn", t("hmult_on", { xp: current.xp, rewards: current.rewards })) : chip("todo", t("hmult_off"))),
    h("p", { class: "card-text" }, t("hmult_text")),
    !k.serverConfig.configured
      ? h("p", { class: "small-print" }, t("host_err_not_configured"))
      : !isPrivate
        ? h("p", { class: "small-print warn-text" }, t("host_err_multipliers_public"))
        : h(
            "div",
            { class: "stack" },
            h("div", { class: "mult-fields" }, field(multXpInput, t("hmult_xp")), field(multRewardsInput, t("hmult_rewards"))),
            xp === null || rewards === null ? h("p", { class: "small-print warn-text" }, t("hmult_range", { max: MAX_MULTIPLIER })) : null,
            h(
              "div",
              { class: "card-row" },
              button(current ? t("hmult_apply") : t("hmult_enable"), () => void api.hostSetMultipliers({ xp: xp!, rewards: rewards! }).then(report).then(hostRefresh), {
                cls: "btn-primary",
                icon: "bolt",
                fk: "host-mult-on",
                disabled: blocked || xp === null || rewards === null || same || (xp === 1 && rewards === 1),
              }),
              button(t("hmult_disable"), () => void api.hostSetMultipliers(null).then(report).then(hostRefresh), { icon: "stop", fk: "host-mult-off", disabled: blocked || current === null }),
            ),
            h("p", { class: "small-print" }, st === "external" ? t("hmult_external") : t("hmult_restart")),
          ),
  );
}

// "Update setup": the project folder compared with GitHub and updated (ReforgedHost's npm run update).
function updateCard(k: HostKit, st: HostServerView["state"], jobRunning: boolean): HTMLElement {
  const u = k.update;
  const gitMissing = k.requirements.some((r) => r.id === "git" && r.state !== "ok");
  const status = u === null ? null : u.state === "up_to_date" ? chip("ok", t("hupd_up_to_date", { head: u.head.slice(0, 7) })) : chip("ok", t("hupd_updated", { n: u.files, head: u.head.slice(0, 7) }));
  return card(
    "",
    h("div", { class: "host-step-head" }, h("h2", { class: "host-step-title" }, t("hupd_title")), status),
    h("p", { class: "card-text" }, t("hupd_text")),
    gitMissing ? h("p", { class: "small-print warn-text" }, t("hupd_git")) : null,
    u?.runtimeChanged ? h("p", { class: "small-print warn-text" }, t("hupd_runtime")) : null,
    h(
      "div",
      { class: "card-row" },
      button(t("hupd_run"), () => void api.hostUpdate().then(report).then(hostRefresh), {
        cls: "btn-primary",
        icon: "refresh",
        fk: "host-update",
        disabled: !k.workspaceOk || jobRunning || st === "external" || st === "starting" || st === "stopping",
      }),
      u?.runtimeChanged ? linkButton(t("hupd_to_build"), () => setHostTab("build"), { icon: "hammer", fk: "host-upd-build" }) : null,
    ),
    h("p", { class: "small-print" }, st === "external" ? t("hupd_external") : t("hupd_note")),
  );
}

function renderHostConsole(): void {
  const el = document.getElementById("host-console");
  if (!el) return;
  const lines = state.hostServer?.lines ?? [];
  renderRegion(el, JSON.stringify([lines, streamer(), state.lang]), () => {
    const pre = h("pre", { class: `host-console${streamer() ? " secret-blur" : ""}`, "aria-live": "off" }, lines.length ? lines.join("\n") : t("hsrv_console_empty"));
    window.requestAnimationFrame(() => (pre.scrollTop = pre.scrollHeight));
    return [pre];
  });
}

function gameDataTab(snap: Snapshot, k: HostKit, steps: ReturnType<typeof hostSteps>): HTMLElement[] {
  const running = k.job?.state === "running";
  const windows = snap.app.platform === "win32";
  const gd = k.gameData;
  const hasExport = k.exports.present || k.exports.inGame;
  const done = k.job?.kind === "generate_gamedata" && k.job.state === "done";
  return [
    card(
      "",
      h("div", { class: "host-step-head" }, h("h2", { class: "host-step-title" }, t("hgd_generate_title")), chip(steps.gameData, steps.gameData === "done" ? t("gamedata_kind_real") : steps.gameData === "warn" ? t("gamedata_kind_synthetic") : t("gamedata_kind_missing"))),
      h("p", { class: "card-text" }, t("hgd_intro")),
      h("div", { class: "card-row" }, chip(gd.backend === "real" ? "ok" : gd.backend === "synthetic" ? "warn" : "todo", t("host_data_backend", { state: tk(`gamedata_kind_${gd.backend}`) })), chip(gd.director === "real" ? "ok" : gd.director === "synthetic" ? "warn" : "todo", t("host_data_director", { state: tk(`gamedata_kind_${gd.director}`) })), chip(hasExport ? "ok" : "todo", hasExport ? t("hgd_export_found") : t("hgd_export_none"))),
      h("p", { class: "card-text" }, t("hgd_generate_text")),
      h("div", { class: "card-row" }, button(t("hgd_generate"), () => hostAct("generate_gamedata"), { cls: "btn-primary btn-big", icon: "database", fk: "host-generate", disabled: running || !k.workspaceOk || !hasExport })),
      done && k.job?.message === "restart" ? h("p", { class: "small-print warn-text" }, t("hgd_restart")) : null,
    ),
    card(
      "",
      h("h2", { class: "card-title" }, t("hgd_export_title")),
      h(
        "ol",
        { class: "steps-list" },
        h("li", {}, h("div", {}, h("div", {}, t("hgd_step_build")), k.builds.exporter ? chip("ok", t("host_built")) : h("div", { class: "card-row" }, linkButton(t("host_tab_build"), () => setHostTab("build"), { icon: "hammer", fk: "host-gd-build" })))),
        h("li", {}, h("div", {}, t("hgd_step_play"))),
        h("li", {}, h("div", {}, h("div", {}, t("hgd_step_inject")), h("div", { class: "card-row" }, button(t("host_data_inject"), () => hostAct("inject_exporter"), { icon: "syringe", fk: "host-exporter", disabled: running || !windows || !k.builds.exporter || !snap.running.game })), !k.builds.exporter ? null : snap.running.game ? null : h("p", { class: "small-print" }, t("host_data_need_game")))),
        h("li", {}, h("div", {}, t("hgd_step_generate"))),
      ),
    ),
    card(
      "",
      h("h2", { class: "card-title" }, t("hgd_pack_title")),
      h("p", { class: "card-text" }, t("hgd_pack_text")),
      h(
        "div",
        { class: "card-row" },
        button(t("hgd_pack_save"), () => hostAct("export_gamedata"), { icon: "upload", fk: "host-pack-save", disabled: running || steps.gameData === "todo" }),
        button(t("host_data_import_pack"), () => void api.hostImportGameData().then(report).then(hostRefresh), { icon: "download", fk: "host-pack", disabled: running || !k.workspaceOk }),
        linkButton(t("hgd_pack_open"), () => hostOpen("gamedata_packs"), { icon: "folder", fk: "host-pack-open" }),
      ),
    ),
    card(
      "",
      h("h2", { class: "card-title" }, t("hgd_example_title")),
      h("p", { class: "card-text" }, t("host_data_step_synthetic")),
      h("div", { class: "card-row" }, button(t("host_data_synthetic"), () => hostAct("synthetic_data"), { icon: "database", fk: "host-synthetic", disabled: running || !k.workspaceOk }), linkButton(t("host_open"), () => hostOpen("game_data"), { icon: "folder", fk: "host-data-open" })),
    ),
  ];
}

function buildTab(snap: Snapshot, k: HostKit, steps: ReturnType<typeof hostSteps>): HTMLElement[] {
  const running = k.job?.state === "running";
  const windows = snap.app.platform === "win32";
  const gameOk = snap.game.root !== null && snap.game.problem !== "not_found" && snap.game.problem !== "not_set" && snap.game.problem !== "wrong_version";
  const tc = k.toolchain;
  return [
    h("p", { class: "card-text" }, t("hbuild_intro")),
    hostStep(
      1,
      t("host_sdk_title"),
      steps.sdk,
      k.sdk.present ? t("host_sdk_ready", { n: k.sdk.functionFiles }) : t("host_sdk_missing"),
      h("p", { class: "card-text" }, t("host_sdk_text")),
      h(
        "ol",
        { class: "steps-list" },
        h("li", {}, h("div", {}, h("div", {}, t("host_sdk_step_dumper")), h("div", { class: "card-row" }, button(t("host_sdk_choose"), () => void api.hostChooseDumper().then(report).then(hostRefresh), { icon: "folder", fk: "host-dumper" }), linkButton(t("host_sdk_get"), () => setHostTab("requirements"), { icon: "list", fk: "host-dumper-get" })), k.dumperDll ? h("p", { class: "small-print selectable" }, secret(k.dumperDll, "host-dumper-path")) : null)),
        h("li", {}, h("div", {}, h("div", {}, t("host_sdk_step_run")), h("div", { class: "card-row" }, button(t("host_sdk_run"), () => hostAct("inject_dumper"), { cls: "btn-primary", icon: "syringe", fk: "host-dump", disabled: !windows || running || !k.dumperDll || !gameOk || snap.running.game })), gameOk ? null : h("p", { class: "small-print" }, t("host_sdk_need_game")))),
        h(
          "li",
          {},
          h(
            "div",
            {},
            h("div", {}, t("host_sdk_step_import")),
            k.dumps.length ? h("p", { class: "small-print" }, t("host_sdk_found", { name: k.dumps[0].name, date: formatDate(k.dumps[0].modified, state.lang) })) : h("p", { class: "small-print" }, t("host_sdk_none")),
            // Mystic Paradox is downloaded first when the server is not set up yet: only a dump and the project folder are needed.
            h(
              "div",
              { class: "card-row" },
              button(t("host_sdk_import"), () => hostAct("import_sdk"), { cls: k.dumps.length && !k.sdk.present ? "btn-primary" : "", icon: "download", fk: "host-import", disabled: running || !k.dumps.length || !k.workspaceOk }),
              linkButton(t("host_sdk_choose_folder"), () => void api.hostImportSdkFolder().then(report).then(hostRefresh), { icon: "folder", fk: "host-import-folder", disabled: running || !k.workspaceOk }),
              linkButton(t("host_open"), () => hostOpen("dumps"), { icon: "folder", fk: "host-dumps-open" }),
            ),
            !k.workspaceOk ? h("p", { class: "small-print warn-text" }, t("host_err_workspace")) : k.upstream.present ? null : h("p", { class: "small-print" }, t("host_sdk_need_setup")),
          ),
        ),
      ),
    ),
    hostStep(
      2,
      t("host_build_title"),
      steps.build,
      steps.build === "done" ? t("host_build_ready") : t("host_build_missing"),
      h("p", { class: "card-text" }, t("host_build_text")),
      h("div", { class: "card-row" }, chip(tc.visualStudio ? "ok" : "todo", tc.visualStudio ?? t("host_build_no_vs")), chip(tc.cargo ? "ok" : "todo", tc.cargo ? "Rust (cargo)" : t("host_build_no_cargo"))),
      h(
        "div",
        { class: "stack" },
        h("div", { class: "settings-row" }, h("div", { class: "settings-row-text" }, h("span", { class: "settings-row-title" }, "MysticParadox.dll"), h("span", { class: "settings-row-sub" }, t(k.builds.runtime ? "host_built" : "host_not_built"))), button(t("host_build_runtime"), () => hostAct("build_runtime"), { icon: "cpu", fk: "host-build-runtime", disabled: running || !windows || !tc.msbuild || !k.sdk.present })),
        h("div", { class: "settings-row" }, h("div", { class: "settings-row-text" }, h("span", { class: "settings-row-title" }, "winmm.dll"), h("span", { class: "settings-row-sub" }, t(k.builds.loader ? "host_built" : "host_not_built"))), button(t("host_build_loader"), () => hostAct("build_loader"), { icon: "cpu", fk: "host-build-loader", disabled: running || !windows || !tc.cargo || !k.workspaceOk })),
        h("div", { class: "settings-row" }, h("div", { class: "settings-row-text" }, h("span", { class: "settings-row-title" }, "CatalogExporter.dll"), h("span", { class: "settings-row-sub" }, t(k.builds.exporter ? "host_built" : "host_not_built"))), button(t("host_build_exporter"), () => hostAct("build_exporter"), { icon: "cpu", fk: "host-build-exporter", disabled: running || !windows || !tc.msbuild || !k.sdk.present })),
      ),
      !tc.visualStudio || !tc.cargo ? h("div", { class: "card-row" }, linkButton(t("host_tab_requirements"), () => setHostTab("requirements"), { icon: "list", fk: "host-build-reqs" })) : null,
    ),
    hostStep(
      3,
      t("host_publish_title"),
      steps.publish,
      steps.publish === "done" ? t("host_publish_ready") : t("host_publish_missing"),
      h("p", { class: "card-text" }, t("host_publish_text")),
      h("div", { class: "card-row" }, button(t("host_publish"), () => hostAct("publish_runtime"), { cls: "btn-primary", icon: "upload", fk: "host-publish", disabled: running || !k.builds.runtime || !k.builds.loader }), linkButton(t("host_open"), () => hostOpen("runtime_publish"), { icon: "folder", fk: "host-publish-open" })),
    ),
  ];
}

function requirementsTab(k: HostKit): HTMLElement[] {
  const byId = new Map(k.requirements.map((r) => [r.id, r]));
  const out: HTMLElement[] = [h("div", { class: "card-row" }, h("p", { class: "card-text" }, t("hreq_intro")), button(t("hreq_recheck"), () => void hostRefresh(), { icon: "refresh", fk: "req-recheck" }))];
  for (const group of REQUIREMENT_GROUPS) {
    const items = group.ids.map((id) => byId.get(id)).filter((r): r is Requirement => r !== undefined);
    if (!items.length) continue;
    out.push(h("h2", { class: "host-group-title" }, t(group.key)), ...items.map(requirementCard));
  }
  if (k.requirements.length === 0) out.push(card("", h("div", { class: "card-row" }, h("div", { class: "spinner" }), t("loading"))));
  return out;
}

function renderHost(): void {
  const container = $("#view-host");
  const snap = state.snap;
  const k = state.host;
  const v = state.hostServer;
  const sig = JSON.stringify([state.lang, state.hostTab, k, v?.state, v?.message, v?.invite, multXpInput.value, multRewardsInput.value, state.consoleOpen, state.setupMode, state.copied, [...state.reqOpen], [...state.reqClosed], snap?.game ?? null, snap?.running.game ?? false, snap?.busy ?? false, snap?.settings.streamerMode, snap?.app.platform]);
  renderRegion(container, sig, () => {
    if (!snap || !k) return [h("div", { class: "page" }, h("h1", { class: "page-title", id: "host-title" }, t("host_title")), card("", h("div", { class: "card-row" }, h("div", { class: "spinner" }), t("loading"))))];
    const steps = hostSteps(k);
    const windows = snap.app.platform === "win32";
    const top: HTMLElement[] = [h("h1", { class: "page-title", id: "host-title" }, t("host_title")), h("p", { class: "page-sub" }, t("host_intro"))];
    if (!windows) top.push(card("card-warn", h("p", { class: "card-text" }, t("host_windows_only"))));
    if (k.job && !(k.job.kind === "create_invite")) {
      top.push(
        card(
          k.job.state === "failed" ? "card-danger" : "card-accent",
          h("div", { class: `host-job ${k.job.state}` }, k.job.state === "running" ? h("div", { class: "spinner" }) : icon(k.job.state === "done" ? "check" : "warning"), h("span", {}, tk(`host_job_${k.job.kind}`), " · ", tk(`host_job_state_${k.job.state}`), k.job.message && k.job.message !== "restart" ? ` · ${k.job.message}` : "")),
          k.job.logFile ? h("div", { class: "card-row" }, linkButton(t("host_open_logs"), () => hostOpen("host_logs"), { icon: "folder", fk: "host-logs" })) : null,
        ),
      );
    }
    const panels: Record<HostTab, () => HTMLElement[]> = {
      server: () => serverTab(snap, k, steps),
      gamedata: () => [...(k.workspaceOk ? [] : [workspaceCard(k, steps)]), ...gameDataTab(snap, k, steps)],
      build: () => [...(k.workspaceOk ? [] : [workspaceCard(k, steps)]), ...buildTab(snap, k, steps)],
      requirements: () => requirementsTab(k),
    };
    const panel = h("div", { class: "host-panel", id: "host-panel", role: "tabpanel", "aria-labelledby": `host-tab-${state.hostTab}` }, ...panels[state.hostTab]());
    window.requestAnimationFrame(() => {
      const el = document.getElementById("host-console");
      if (el) forceRender(el);
      renderHostConsole();
    });
    return [h("div", { class: "page page-wide" }, ...top, h("div", { class: "settings-layout" }, hostTabs(), panel))];
  });
  renderHostConsole();
}

// ------------------------------------------------------------------ settings page

function settingsRow(title: string, sub: string | Node | null, ...actions: (HTMLElement | null)[]): HTMLElement {
  return h("div", { class: "settings-row" }, h("div", { class: "settings-row-text" }, h("span", { class: "settings-row-title" }, title), sub ? h("span", { class: "settings-row-sub" }, sub) : null), h("div", { class: "settings-actions" }, ...actions));
}

interface DropdownOption<T> {
  value: T;
  label: string;
  lang?: string;
}

// A select in the launcher's own style (Dauntless Revived's): a button and a listbox, with the keys
// a select has: Up/Down/Home/End move, Enter/Space choose, Escape and Tab close, a letter jumps.
function dropdown<T extends string | number>(opts: { id: string; fk: string; value: T; options: DropdownOption<T>[]; onChange: (v: T) => void; disabled?: boolean }): HTMLElement {
  const current = opts.options.find((o) => o.value === opts.value) ?? opts.options[0];
  const listId = `${opts.id}-list`;
  const btn = h("button", { type: "button", class: "select dropdown-button", id: opts.id, "data-fk": opts.fk, "aria-haspopup": "listbox", "aria-expanded": "false", "aria-controls": listId, disabled: opts.disabled }, current.label);
  if (current.lang) btn.setAttribute("lang", current.lang);
  const list = h("ul", { class: "dropdown-list", id: listId, role: "listbox", tabindex: "-1", hidden: true });
  const items = opts.options.map((o, i) => {
    const li = h("li", { class: "dropdown-option", role: "option", id: `${opts.id}-opt-${i}`, "aria-selected": o.value === current.value ? "true" : "false", lang: o.lang }, o.label);
    li.addEventListener("mousedown", (e) => e.preventDefault());
    li.addEventListener("click", () => choose(i));
    li.addEventListener("mousemove", () => highlight(i));
    list.appendChild(li);
    return li;
  });
  const wrap = h("div", { class: "dropdown" }, btn, list);
  let active = Math.max(0, opts.options.indexOf(current));

  function highlight(i: number): void {
    active = (i + items.length) % items.length;
    items.forEach((li, j) => li.classList.toggle("active", j === active));
    list.setAttribute("aria-activedescendant", items[active].id);
    items[active].scrollIntoView({ block: "nearest" });
  }
  function onOutside(e: MouseEvent): void {
    if (!wrap.contains(e.target as Node)) close(false);
  }
  function openList(): void {
    if (opts.disabled || !list.hidden) return;
    list.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    const below = window.innerHeight - btn.getBoundingClientRect().bottom;
    wrap.classList.toggle("dropdown-up", below < list.offsetHeight + 12);
    highlight(Math.max(0, opts.options.indexOf(current)));
    list.focus();
    document.addEventListener("mousedown", onOutside, true);
  }
  function close(refocus: boolean): void {
    if (list.hidden) return;
    list.hidden = true;
    btn.setAttribute("aria-expanded", "false");
    document.removeEventListener("mousedown", onOutside, true);
    if (refocus) btn.focus();
  }
  function choose(i: number): void {
    close(true);
    const o = opts.options[i];
    if (o && o.value !== current.value) opts.onChange(o.value);
  }

  btn.addEventListener("click", () => (list.hidden ? openList() : close(true)));
  btn.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      openList();
    }
  });
  list.addEventListener("keydown", (e) => {
    const moves: Record<string, () => number> = { ArrowDown: () => active + 1, ArrowUp: () => active - 1, Home: () => 0, End: () => items.length - 1 };
    if (moves[e.key]) {
      e.preventDefault();
      highlight(moves[e.key]());
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      choose(active);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === "Tab") {
      close(false);
    } else if (e.key.length === 1) {
      const key = e.key.toLowerCase();
      const next = opts.options.findIndex((o, j) => j > active && o.label.toLowerCase().startsWith(key));
      const first = opts.options.findIndex((o) => o.label.toLowerCase().startsWith(key));
      if (next >= 0 || first >= 0) highlight(next >= 0 ? next : first);
    }
  });
  list.addEventListener("blur", (e) => {
    if (e.relatedTarget !== btn) close(false);
  });
  return wrap;
}

// A switch slides its knob first and saves after, because saving redraws the page with a new switch
// already in place and the slide would never be seen.
function toggle(id: string, labelId: string, on: boolean, save: (on: boolean) => void): HTMLElement {
  const sw = h("button", { type: "button", class: "switch", role: "switch", "aria-checked": on ? "true" : "false", "aria-labelledby": labelId, "data-fk": id });
  sw.addEventListener("click", () => {
    const next = sw.getAttribute("aria-checked") !== "true";
    sw.setAttribute("aria-checked", next ? "true" : "false");
    window.setTimeout(() => save(next), 220);
  });
  return sw;
}

// A setting's long explanation sits behind a small info button: a tooltip on hover and on keyboard
// focus, and the button's accessible name, so the page shows only the setting's name.
function infoTip(text: string): HTMLElement {
  return h("button", { type: "button", class: "info-tip", "aria-label": text, "data-tip": text }, icon("info"));
}

function infoRow(title: HTMLElement, text: string, control: HTMLElement): HTMLElement {
  return h("div", { class: "settings-row" }, h("div", { class: "settings-row-text" }, h("div", { class: "settings-row-head" }, title, infoTip(text))), control);
}

function renderSettings(): void {
  const container = $("#view-settings");
  const snap = state.snap;
  if (!snap) return;
  const sig = JSON.stringify([state.lang, state.settingsTab, snap.settings, snap.game, snap.runtime, snap.account, snap.server, snap.savedServers, snap.app, snap.phase, snap.busy, !!snap.task]);
  renderRegion(container, sig, () => {
    const busy = snap.busy || snap.task !== null || snap.running.game;

    const game = card(
      "settings-section",
      h("h2", { class: "card-title" }, t("set_game")),
      settingsRow(
        t("set_folder"),
        snap.game.root ? secret(snap.game.root, "game-root") : t("set_folder_none"),
        button(t("set_change_folder"), () => void api.chooseGameFolder().then(report), { icon: "folder", fk: "set-dir", disabled: busy }),
        snap.game.root ? button(t("set_open_folder"), () => void api.openGameFolder(), { fk: "set-open" }) : null,
      ),
      settingsRow(t("set_verify"), snap.game.exeVerified ? t("set_verify_ok") : t("set_verify_text"), button(t("set_verify_button"), () => void api.verifyGame().then(report), { icon: "refresh", fk: "set-verify", disabled: busy || !snap.game.root })),
      infoRow(
        h("label", { class: "settings-row-title", for: "game-language-select" }, t("set_game_language")),
        t("set_game_language_text"),
        dropdown<GameLanguage>({
          id: "game-language-select",
          fk: "game-language",
          value: snap.settings.gameLanguage,
          options: GAME_LANGUAGES.map((g) => (g === "auto" ? { value: g, label: t("game_lang_auto") } : { value: g, label: GAME_LANGUAGE_NAMES[g], lang: g })),
          onChange: (gameLanguage) => void api.setSettings({ gameLanguage }),
        }),
      ),
    );

    const graphics = card(
      "settings-section",
      h("h2", { class: "card-title" }, t("set_graphics")),
      infoRow(
        h("label", { class: "settings-row-title", for: "gfx-select" }, t("set_graphics_level")),
        t("set_graphics_text"),
        dropdown<GraphicsPreset>({ id: "gfx-select", fk: "gfx", value: snap.settings.graphics, options: GRAPHICS_PRESETS.map((g) => ({ value: g, label: t(graphicsKey(g)) })), onChange: (graphics) => void api.setSettings({ graphics }) }),
      ),
      infoRow(h("span", { class: "settings-row-title", id: "windowed-label" }, t("set_windowed")), t("set_windowed_help"), toggle("windowed", "windowed-label", snap.settings.windowed, (on) => void api.setSettings({ windowed: on }))),
    );

    const account = card(
      "settings-section",
      h("h2", { class: "card-title" }, t("set_account")),
      snap.server && snap.account.hasKey
        ? settingsRow(
            t("set_signed_in", { user: snap.account.username ?? "?", name: serverName() }),
            t("set_key_note"),
            button(t("set_backup"), () => void api.saveKeyBackup().then(report), { icon: "key", fk: "set-backup" }),
            button(t("set_logout"), () => showModal({ kind: "logout" }), { cls: "btn-danger", fk: "set-logout", disabled: busy }),
          )
        : settingsRow(t("set_not_signed"), null),
      snap.server ? settingsRow(t("set_server_current"), h("span", {}, `${serverName()} (`, secret(`${snap.server.host}:${snap.server.port}`, "server-address"), ")"), button(t("set_add_server"), () => showModal({ kind: "leave" }), { fk: "set-leave", disabled: busy })) : null,
    );
    const otherServers = snap.savedServers.some((sv) => !sv.current)
      ? card(
          "settings-section",
          h("div", { class: "settings-row-head card-title" }, h("h2", { class: "card-title-text" }, t("saved_other_title")), infoTip(t("saved_other_text"))),
          ...snap.savedServers
            .filter((sv) => !sv.current)
            .map((sv) =>
              settingsRow(
                sv.name,
                savedServerSub(sv),
                button(t("saved_switch_to"), () => void switchTo(sv.id), { cls: "btn-primary", fk: `set-saved-${sv.id}`, disabled: busy }),
                button(t("saved_remove"), () => showModal({ kind: "remove_server", id: sv.id, name: sv.name }), { icon: "close", fk: `set-saved-rm-${sv.id}`, disabled: busy }),
              ),
            ),
        )
      : null;

    const langRow = h("div", { class: "lang-switch", role: "group", "aria-label": t("set_language") });
    for (const l of LANGUAGES) {
      const b = h("button", { type: "button", class: "lang-btn", lang: l, "aria-pressed": snap.settings.language === l ? "true" : "false", "data-fk": `set-lang-${l}` }, LANGUAGE_NAMES[l]);
      b.addEventListener("click", () => void api.setSettings({ language: l }));
      langRow.appendChild(b);
    }
    const language = card("settings-section", h("h2", { class: "card-title" }, t("set_language")), langRow);

    const advanced = card(
      "settings-section",
      h("h2", { class: "card-title" }, t("set_advanced")),
      infoRow(h("span", { class: "settings-row-title", id: "host-tools-label" }, t("set_host_tools")), t("set_host_tools_text"), toggle("host-tools", "host-tools-label", snap.settings.hostTools, (on) => void api.setSettings({ hostTools: on }))),
      infoRow(
        h("label", { class: "settings-row-title", for: "profile-select" }, t("set_runtime_profile")),
        t("set_runtime_profile_text"),
        dropdown<RuntimeProfile>({ id: "profile-select", fk: "profile", value: snap.settings.runtimeProfile, options: RUNTIME_PROFILES.map((p) => ({ value: p, label: tk(`profile_${p}`) })), onChange: (runtimeProfile) => void api.setSettings({ runtimeProfile }) }),
      ),
      settingsRow(
        t("set_local_runtime"),
        snap.runtime.localDir ? secret(snap.runtime.localDir, "local-runtime") : t("set_local_runtime_text"),
        button(t("set_local_runtime_choose"), () => void api.chooseLocalRuntime().then(report), { icon: "folder", fk: "local-runtime", disabled: busy }),
        snap.runtime.localDir ? button(t("set_local_runtime_clear"), () => void api.clearLocalRuntime().then(report), { icon: "close", fk: "local-runtime-clear", disabled: busy }) : null,
      ),
      infoRow(h("span", { class: "settings-row-title", id: "streamer-label" }, t("set_streamer")), t("set_streamer_help"), toggle("streamer", "streamer-label", streamer(), (on) => void api.setSettings({ streamerMode: on }))),
      settingsRow(t("set_logs"), t("set_logs_text"), button(t("set_logs_open"), () => void api.openLogsFolder(), { icon: "folder", fk: "logs" })),
    );

    const about = card(
      "settings-section",
      h("h2", { class: "card-title" }, t("set_about")),
      h("p", { class: "about-text" }, t("about_version", { v: snap.app.version })),
      h("p", { class: "about-text" }, t("about_license")),
      h("div", { class: "card-row" }, linkButton(t("about_source"), () => open("project_source"), { icon: "external", fk: "about-src" }), linkButton(t("about_license_link"), () => open("project_license"), { icon: "external", fk: "about-lic" }), linkButton(t("about_issues"), () => open("project_issues"), { icon: "external", fk: "about-issues" })),
      h("div", { class: "card-row" }, h("span", { class: "small-print" }, t("about_credits")), linkButton(t("nav_credits"), () => setView("credits"), { fk: "about-credits" })),
      h("p", { class: "small-print" }, t("about_disclaimer")),
      h("p", { class: "small-print" }, snap.app.packaged ? t("about_auto_updates") : t("about_dev_build")),
      snap.app.updateReady ? h("div", { class: "card-row" }, button(t("update_restart"), () => void api.installUpdate(), { cls: "btn-primary", fk: "about-update", disabled: snap.running.game })) : null,
    );

    const panels: Record<SettingsTab, (HTMLElement | null)[]> = {
      game: [game],
      graphics: [graphics],
      account: [account, otherServers],
      launcher: [language, advanced, about],
    };
    const tabs = h("div", { class: "settings-tabs", role: "tablist", "aria-label": t("set_title") });
    for (const tab of SETTINGS_TABS) {
      const on = state.settingsTab === tab;
      const b = h("button", { type: "button", class: "settings-tab", role: "tab", id: `set-tab-${tab}`, "aria-selected": on ? "true" : "false", "aria-controls": "settings-panel", tabindex: on ? "0" : "-1", "data-fk": `set-tab-${tab}`, title: t(`set_tab_${tab}` as StringKey) }, icon(SETTINGS_TAB_ICONS[tab]), h("span", { class: "settings-tab-label" }, t(`set_tab_${tab}` as StringKey)));
      b.addEventListener("click", () => {
        state.settingsTab = tab;
        renderSettings();
      });
      b.addEventListener("keydown", (e) => {
        const i = SETTINGS_TABS.indexOf(tab);
        const next = e.key === "ArrowRight" ? SETTINGS_TABS[(i + 1) % SETTINGS_TABS.length] : e.key === "ArrowLeft" ? SETTINGS_TABS[(i + SETTINGS_TABS.length - 1) % SETTINGS_TABS.length] : null;
        if (!next) return;
        e.preventDefault();
        state.settingsTab = next;
        renderSettings();
        document.getElementById(`set-tab-${next}`)?.focus();
      });
      tabs.appendChild(b);
    }
    const panel = h("div", { class: "settings-panel", id: "settings-panel", role: "tabpanel", "aria-labelledby": `set-tab-${state.settingsTab}` }, ...panels[state.settingsTab]);
    return [h("div", { class: "page page-wide" }, h("h1", { class: "page-title", id: "settings-title" }, t("set_title")), h("div", { class: "settings-layout" }, tabs, panel))];
  });
}

// ------------------------------------------------------------------ credits page

const ROLE_KEY: Record<CreditRole, StringKey> = {
  maintainer: "credits_role_maintainer",
  creator: "credits_role_creator",
  contributor: "credits_role_contributor",
};

function creditPeople(people: readonly CreditPerson[]): HTMLElement {
  return h(
    "ul",
    { class: "credit-list" },
    ...people.map((p) =>
      h(
        "li",
        { class: "credit" },
        h("span", { class: `credit-avatar role-${p.role}`, "aria-hidden": "true" }, Array.from(p.name)[0]?.toUpperCase() ?? "?"),
        h(
          "div",
          { class: "credit-text" },
          h("div", { class: "credit-head" }, h("span", { class: "credit-name" }, p.name), h("span", { class: "credit-handle" }, `@${p.github}`), h("span", { class: "credit-role" }, t(ROLE_KEY[p.role]))),
          h("p", { class: "credit-note" }, localized(p.note, state.lang)),
        ),
      ),
    ),
  );
}

function creditSoftware(): HTMLElement {
  return h(
    "ul",
    { class: "credit-list" },
    ...SOFTWARE.map((sw) =>
      h(
        "li",
        { class: "software" },
        h("div", { class: "credit-head" }, h("span", { class: "credit-name" }, sw.name), h("span", { class: "credit-handle" }, localized(sw.author, state.lang)), h("span", { class: "software-license" }, sw.license ?? t("credits_no_license"))),
        h("p", { class: "credit-note" }, localized(sw.note, state.lang)),
      ),
    ),
  );
}

function renderCredits(): void {
  const container = $("#view-credits");
  renderRegion(container, JSON.stringify([state.lang]), () => [
    h(
      "div",
      { class: "page credits-page" },
      h("h1", { class: "page-title", id: "credits-title" }, t("credits_title")),
      h("p", { class: "page-sub" }, t("credits_intro")),
      ...PROJECTS.map((p) =>
        card("settings-section", h("h2", { class: "card-title" }, p.name), h("p", { class: "card-text" }, localized(p.text, state.lang)), creditPeople(p.people), h("div", { class: "card-row" }, linkButton(t("credits_source", { name: p.name }), () => open(p.link), { icon: "external", fk: `cr-${p.link}` }))),
      ),
      card("settings-section", h("h2", { class: "card-title" }, t("credits_software")), h("p", { class: "card-text" }, t("credits_software_text")), creditSoftware()),
      card(
        "settings-section",
        h("h2", { class: "card-title" }, t("credits_license_title")),
        h("p", { class: "card-text" }, t("credits_license_text")),
        h("div", { class: "card-row" }, linkButton(t("github_link"), () => open("project_source"), { icon: "external", fk: "cr-source" }), linkButton(t("about_license_link"), () => open("project_license"), { icon: "external", fk: "cr-license" })),
        h("p", { class: "small-print" }, t("credits_phoenix")),
      ),
    ),
  ]);
}

// ------------------------------------------------------------------ banners

function renderBanners(): void {
  const container = $("#banners");
  const snap = state.snap;
  const error = snap?.lastError && snap.lastError.code !== "cancelled" && !(snap.phase === "register" && snap.lastError.code === "key_rejected") ? snap.lastError : state.localError;
  const sig = JSON.stringify([state.lang, error, snap?.notice ?? null, snap?.app.updateReady ?? false, snap?.running.game ?? false]);
  renderRegion(container, sig, () => {
    const out: HTMLElement[] = [];
    if (error) {
      out.push(
        h(
          "div",
          { class: "banner banner-error", role: "alert" },
          icon("warning"),
          h("span", { class: "banner-text" }, errorText(error)),
          linkButton(t("err_dismiss"), () => {
            state.localError = null;
            void api.dismissError();
            renderBanners();
          }, { fk: "err-dismiss" }),
        ),
      );
    }
    if (snap?.notice) {
      out.push(h("div", { class: "banner banner-notice", role: "status" }, icon("info"), h("span", { class: "banner-text" }, tk(`notice_${snap.notice}`)), linkButton(t("err_dismiss"), () => void api.dismissNotice(), { fk: "notice-dismiss" })));
    }
    if (snap?.app.updateReady) {
      out.push(h("div", { class: "banner banner-update", role: "status" }, icon("refresh"), h("span", { class: "banner-text" }, t("update_ready")), button(t("update_restart"), () => void api.installUpdate(), { cls: "btn-primary", fk: "update-restart", disabled: snap.running.game })));
    }
    return out;
  });
}

// ------------------------------------------------------------------ rail, titlebar, views

function navItems(): { view: View; icon: IconName; key: StringKey }[] {
  const items: { view: View; icon: IconName; key: StringKey }[] = [
    { view: "play", icon: "play", key: "nav_play" },
    { view: "news", icon: "news", key: "nav_news" },
    { view: "server", icon: "server", key: "nav_server" },
  ];
  if (state.snap?.settings.hostTools) items.push({ view: "host", icon: "anvil", key: "nav_host" });
  items.push({ view: "settings", icon: "settings", key: "nav_settings" });
  return items;
}

function renderRail(): void {
  const snap = state.snap;
  const nav = $("#nav");
  const unread = state.news.length > 0 && !state.newsSeen;
  const items = navItems();
  renderRegion(nav, JSON.stringify([state.lang, state.view, unread, items.map((i) => i.view)]), () =>
    items.map((n) => {
      const b = h("button", { type: "button", class: "nav-item", "aria-current": state.view === n.view ? "page" : undefined, "data-fk": `nav-${n.view}` }, icon(n.icon), t(n.key), n.view === "news" && unread ? h("img", { class: "nav-badge", src: newsBadge, alt: t("news_new"), draggable: "false" }) : null);
      b.addEventListener("click", () => setView(n.view));
      return h("li", {}, b);
    }),
  );
  const chipEl = $("#account-chip");
  const user = snap?.account.hasKey ? snap.account.username : null;
  // With other servers saved, the chip is also the server switcher: a click lists them, one more click joins.
  const others = snap?.savedServers.filter((sv) => !sv.current) ?? [];
  const locked = !snap || snap.busy || snap.task !== null || snap.running.game;
  if (others.length === 0 || locked) state.serverMenu = false;
  renderRegion(chipEl, JSON.stringify([state.lang, user, snap?.server?.name ?? null, snap?.status?.name ?? null, others, locked, state.serverMenu, snap?.settings.streamerMode]), () => {
    const inner = [
      h("span", { class: `avatar${user ? "" : " empty"}`, "aria-hidden": "true" }, user ? Array.from(user)[0].toUpperCase() : "?"),
      h("span", { class: "account-text" }, h("span", { class: "account-label" }, snap?.server ? serverName() : t("rail_not_joined")), h("span", { class: "account-name" }, user ?? t("rail_not_signed_in"))),
    ];
    if (others.length === 0) return inner;
    const toggleBtn = h("button", { type: "button", class: "account-switch", "data-fk": "server-switch", "aria-haspopup": "menu", "aria-expanded": state.serverMenu ? "true" : "false", title: t("saved_switch"), disabled: locked }, ...inner);
    toggleBtn.addEventListener("click", () => {
      state.serverMenu = !state.serverMenu;
      renderRail();
    });
    const menu = h(
      "ul",
      { class: "server-menu", role: "menu", "aria-label": t("saved_switch"), hidden: !state.serverMenu },
      h("li", { class: "server-menu-head", role: "presentation" }, t("saved_switch")),
      ...others.map((sv) => {
        const item = h("button", { type: "button", class: "server-menu-item", role: "menuitem", "data-fk": `switch-${sv.id}` }, h("span", { class: "server-menu-name" }, sv.name), h("span", { class: "server-menu-sub" }, savedServerSub(sv)));
        item.addEventListener("click", () => {
          state.serverMenu = false;
          renderRail();
          void switchTo(sv.id);
        });
        return h("li", { role: "none" }, item);
      }),
    );
    return [toggleBtn, menu];
  });
  for (const b of Array.from(document.querySelectorAll<HTMLButtonElement>("#lang-switch .lang-btn"))) {
    b.setAttribute("aria-pressed", b.dataset.lang === state.lang ? "true" : "false");
  }
  const credits = $("#credits-btn");
  if (state.view === "credits") credits.setAttribute("aria-current", "page");
  else credits.removeAttribute("aria-current");
}

function setView(v: View): void {
  const changed = state.view !== v;
  if (v === "credits" && state.view !== "credits") {
    const from = document.activeElement;
    state.creditsReturn = { view: state.view, focus: from instanceof HTMLElement && from !== document.body ? from : null };
  }
  state.view = v;
  if (v === "news") state.newsSeen = true;
  if (v === "host") {
    void api.hostGet().then((k) => ((state.host = k), renderHost()));
    void api.hostServerGet().then((sv) => ((state.hostServer = sv), renderHost()));
  }
  for (const view of VIEWS) $(`#view-${view}`).hidden = view !== v;
  $("#actionbar").hidden = false;
  // The Server page shows everything the side panel shows, in full; the Host and Credits pages have
  // long content and get the room too.
  $(".world").classList.toggle("no-panel", v === "server" || v === "credits" || v === "host");
  renderAll();
  if (changed && !reducedMotion.matches) {
    const selected = $(`#view-${v}`);
    selected.classList.remove("view-entering");
    void selected.offsetWidth;
    selected.classList.add("view-entering");
    window.setTimeout(() => selected.classList.remove("view-entering"), 520);
  }
  const headingEl = document.querySelector<HTMLElement>(`#view-${v} h1`);
  headingEl?.setAttribute("tabindex", "-1");
  if (v !== "play") headingEl?.focus({ preventScroll: true });
}

function leaveCredits(): void {
  const back = state.creditsReturn ?? { view: "play" as View, focus: null };
  state.creditsReturn = null;
  const fk = back.focus?.getAttribute("data-fk") ?? null;
  setView(back.view);
  const again = back.focus?.isConnected ? back.focus : fk ? document.querySelector<HTMLElement>(`#view-${back.view} [data-fk="${CSS.escape(fk)}"]`) : null;
  (again ?? $("#credits-btn")).focus({ preventScroll: true });
}

function applyStaticI18n(): void {
  document.documentElement.lang = state.lang;
  document.title = t("app_title");
  for (const el of Array.from(document.querySelectorAll<HTMLElement>("[data-i18n]"))) el.textContent = tk(el.dataset.i18n ?? "");
  for (const el of Array.from(document.querySelectorAll<HTMLElement>("[data-i18n-aria]"))) el.setAttribute("aria-label", tk(el.dataset.i18nAria ?? ""));
  inviteInput.placeholder = t("join_placeholder");
  usernameInput.placeholder = t("reg_placeholder");
  keyInput.placeholder = t("reg_key_placeholder");
  gamePathInput.placeholder = t("locate_path_placeholder");
  $("#win-max").setAttribute("aria-label", t(state.maximized ? "window_restore" : "window_maximize"));
  $("#github-btn").title = t("github_link");
}

function renderAll(): void {
  applyStaticI18n();
  // The rail says which game the current server runs.
  const tileVersion = document.querySelector(".tile-version");
  if (tileVersion) tileVersion.textContent = state.snap?.server?.game === "revived" ? t("edition_revived") : t("tile_sub");
  updateInviteFeedback();
  updateUsernameFeedback();
  // The Host page is hidden again when its switch is turned off.
  if (state.view === "host" && !state.snap?.settings.hostTools) {
    state.view = "settings";
    for (const view of VIEWS) $(`#view-${view}`).hidden = view !== "settings";
    $(".world").classList.remove("no-panel");
  }
  renderRail();
  renderBanners();
  if (state.view === "play") renderPlay();
  if (state.view === "news") renderNews();
  if (state.view === "server") renderServer();
  if (state.view === "host") renderHost();
  if (state.view === "settings") renderSettings();
  if (state.view === "credits") renderCredits();
  renderActionBar();
  renderPanel();
}

// ------------------------------------------------------------------ modals

function showModal(m: Modal): void {
  state.modalReturnFocus = document.activeElement as HTMLElement | null;
  state.modal = m;
  renderModal();
}

function closeModal(): void {
  state.modal = null;
  renderModal();
  state.modalReturnFocus?.focus?.();
}

function renderModal(): void {
  const root = $("#modal-root");
  const m = state.modal;
  if (!m) {
    root.hidden = true;
    root.replaceChildren();
    return;
  }
  const name = serverName();
  let title = "";
  let text: HTMLElement[] = [];
  let confirmLabel = "";
  let confirmCls = "btn-primary";
  // Resolves to the next modal to show (a changed certificate), or null to close.
  let onConfirm: () => Promise<Modal | null> = async () => null;
  switch (m.kind) {
    case "invite":
      title = t("link_title", { name: m.name });
      text = [h("p", { class: "modal-text" }, t("link_text", { name: m.name, host: hideText(m.host) })), h("div", {}, modeBadge(m.mode))];
      if (m.fp) text.push(h("p", { class: "modal-text mono" }, t("link_fingerprint", { fp: hideText(shortFingerprint(m.fp)) })));
      if (state.snap?.server) text.push(h("p", { class: "modal-text" }, t("link_switch_note")));
      confirmLabel = t("link_join");
      onConfirm = async () => {
        const r = await api.submitInvite(m.link);
        if (!r.ok && r.error.code === "cert_changed") return inviteModal(m.link);
        await report(r);
        setView("play");
        return null;
      };
      break;
    case "server_moved":
      title = t("moved_title", { name: m.name });
      text = [
        h("p", { class: "modal-text" }, t("moved_text", { name: m.name, old: hideText(m.oldHost), new: hideText(m.newHost) })),
        h("p", { class: "modal-text modal-warn" }, icon("info"), h("span", {}, t("moved_advice"))),
      ];
      confirmLabel = t("moved_confirm");
      onConfirm = async () => {
        const r = await api.submitInvite(m.link, false, true);
        if (r.ok && state.inviteText.trim() === m.link) clearInviteInput();
        await report(r);
        setView("play");
        return null;
      };
      break;
    case "cert_changed":
      title = t("certchg_title");
      text = [
        h("p", { class: "modal-text" }, t("certchg_text", { host: hideText(m.host) })),
        h("dl", { class: "fp-compare" }, h("dt", {}, t("certchg_old")), h("dd", { class: "fingerprint" }, m.oldFp ? groupedFingerprint(m.oldFp) : t("certchg_unknown")), h("dt", {}, t("certchg_new")), h("dd", { class: "fingerprint fp-new" }, groupedFingerprint(m.newFp))),
        h("p", { class: "modal-text modal-warn" }, icon("warning"), h("span", {}, t("certchg_advice"))),
      ];
      confirmLabel = t("certchg_confirm");
      confirmCls = "btn-danger";
      onConfirm = async () => {
        const r = await api.submitInvite(m.link, true);
        if (r.ok && state.inviteText.trim() === m.link) clearInviteInput();
        await report(r);
        setView("play");
        return null;
      };
      break;
    case "trust_runtime": {
      const snap = state.snap;
      title = t("trust_title", { name });
      text = [h("p", { class: "modal-text" }, t("trust_text", { name })), h("p", { class: "modal-text" }, t("trust_detail")), ...(snap ? [runtimeFiles(snap)] : []), h("p", { class: "modal-text modal-warn" }, icon("shield"), h("span", {}, t("trust_advice")))];
      confirmLabel = t("trust_confirm");
      onConfirm = async () => {
        await report(await api.trustRuntime());
        return null;
      };
      break;
    }
    case "logout":
      title = t("logout_title", { name });
      text = [h("p", { class: "modal-text" }, t("logout_text"))];
      confirmLabel = t("logout_confirm");
      confirmCls = "btn-danger";
      onConfirm = async () => {
        await report(await api.logout());
        return null;
      };
      break;
    case "leave":
      title = t("leave_title");
      text = [h("p", { class: "modal-text" }, t("leave_text", { name }))];
      confirmLabel = t("leave_confirm");
      onConfirm = async () => {
        const r = await api.forgetServer();
        if (!r.ok) {
          await report(r);
          return null;
        }
        setView("play");
        return null;
      };
      break;
    case "remove_server":
      title = t("remove_server_title", { name: m.name });
      text = [h("p", { class: "modal-text" }, t("remove_server_text", { name: m.name }))];
      confirmLabel = t("remove_server_confirm");
      onConfirm = async () => {
        await report(await api.removeSavedServer(m.id));
        return null;
      };
      break;
  }
  const cancel = button(m.kind === "invite" ? t("link_cancel") : t("cancel"), () => closeModal(), { fk: "modal-cancel" });
  const confirm = button(
    confirmLabel,
    () => {
      confirm.disabled = true;
      void onConfirm()
        .catch(() => null)
        .then((next) => {
          if (next) {
            state.modal = next;
            renderModal();
          } else closeModal();
        });
    },
    { cls: confirmCls, fk: "modal-confirm" },
  );
  const danger = m.kind === "cert_changed";
  const dialog = h(
    "div",
    { class: `modal${danger ? " modal-danger" : ""}`, role: danger ? "alertdialog" : "dialog", "aria-modal": "true", "aria-labelledby": "modal-title" },
    h("h2", { class: "modal-title", id: "modal-title" }, danger || m.kind === "trust_runtime" ? icon("shield") : null, title),
    ...text,
    h("div", { class: "modal-actions" }, cancel, confirm),
  );
  root.replaceChildren(dialog);
  root.hidden = false;
  // Destructive or risky choices never get the default focus.
  (m.kind === "logout" || m.kind === "cert_changed" || m.kind === "remove_server" || m.kind === "trust_runtime" ? cancel : confirm).focus();
}

window.addEventListener("mousedown", (e) => {
  if (state.serverMenu && !$("#account-chip").contains(e.target as Node)) {
    state.serverMenu = false;
    renderRail();
  }
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && state.serverMenu) {
    state.serverMenu = false;
    renderRail();
    document.querySelector<HTMLButtonElement>(".account-switch")?.focus();
  }
});

document.addEventListener("keydown", (e) => {
  if (!state.modal) {
    if (e.key === "Escape" && state.view === "credits" && !e.defaultPrevented) {
      e.preventDefault();
      leaveCredits();
    }
    return;
  }
  if (e.key === "Escape") {
    e.preventDefault();
    closeModal();
    return;
  }
  if (e.key === "Tab") {
    const focusable = Array.from(document.querySelectorAll<HTMLElement>("#modal-root button:not(:disabled)"));
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    } else if (!focusable.includes(document.activeElement as HTMLElement)) {
      e.preventDefault();
      first.focus();
    }
  }
});

// The modal for an invite: the plain "Join?" question, or the certificate warning when this PC's
// key for that public server belongs to another certificate.
async function inviteModal(link: string): Promise<Modal | null> {
  const r = parseInvite(link);
  if (!r.ok) return null;
  const inv = r.invite;
  const check = await api.checkInvite(link);
  if (check.ok && check.certificateChanged && inv.fp) return { kind: "cert_changed", link, name: inv.name, host: inv.host, oldFp: check.previousFingerprint, newFp: inv.fp };
  if (check.ok && check.movedFrom) return { kind: "server_moved", link, name: inv.name, oldHost: check.movedFrom, newHost: inv.host };
  return { kind: "invite", link, name: inv.name, host: inv.host, mode: inv.mode, fp: inv.fp };
}

async function checkInviteLink(): Promise<void> {
  const link = await api.takeInviteLink();
  if (!link) return;
  const m = await inviteModal(link);
  if (m) showModal(m);
}

// ------------------------------------------------------------------ hero art (the host's art pack)

function showArt(index: number): void {
  const list = state.branding.backgrounds.filter((b) => ART_URL.test(b.url));
  const layers = [$("#art-a"), $("#art-b")];
  const credit = $("#art-credit");
  if (list.length === 0) {
    layers.forEach((l) => l.classList.remove("show"));
    credit.textContent = "";
    $("#hero").classList.remove("has-art");
    return;
  }
  const item = list[index % list.length];
  const next = layers[1 - state.artLayer];
  const img = new Image();
  img.onload = () => {
    next.style.backgroundImage = `url("${item.url}")`;
    next.classList.add("show");
    layers[state.artLayer].classList.remove("show");
    state.artLayer = 1 - state.artLayer;
    $("#hero").classList.add("has-art");
    credit.textContent = item.credit ? t("art_credit", { credit: item.credit }) : "";
  };
  img.src = item.url;
}

function startArt(): void {
  window.clearInterval(state.artTimer);
  state.artIndex = 0;
  showArt(0);
  const count = state.branding.backgrounds.length;
  if (count > 1 && !reducedMotion.matches) {
    state.artTimer = window.setInterval(() => {
      state.artIndex++;
      showArt(state.artIndex);
    }, 14000);
  }
  const accent = state.branding.accent;
  if (accent && /^#[0-9a-f]{6}$/.test(accent)) document.documentElement.style.setProperty("--host-accent", accent);
  else document.documentElement.style.removeProperty("--host-accent");
}

// ------------------------------------------------------------------ data flow

async function loadExtras(snap: Snapshot): Promise<void> {
  if (!snap.server || snap.phase === "join" || snap.phase === "connect") return;
  const key = `${snap.server.mode}|${snap.server.host}:${snap.server.port}`;
  const empty = state.news.length === 0 && state.branding.backgrounds.length === 0;
  if (key === state.extrasFor && !(empty && Date.now() - state.extrasAt > 5000)) return;
  const changedServer = key !== state.extrasFor;
  state.extrasFor = key;
  state.extrasAt = Date.now();
  const [news, branding] = await Promise.all([api.getNews(), api.getBranding()]);
  const newsChanged = JSON.stringify(news) !== JSON.stringify(state.news);
  const artChanged = JSON.stringify(branding) !== JSON.stringify(state.branding);
  state.news = news;
  if (changedServer) state.newsSeen = false;
  if (artChanged) {
    state.branding = branding;
    startArt();
  }
  if (newsChanged || artChanged) renderAll();
}

function onSnapshot(snap: Snapshot): void {
  const langChanged = snap.settings.language !== state.lang;
  const streamerChanged = snap.settings.streamerMode !== state.snap?.settings.streamerMode;
  const serverChanged = JSON.stringify(snap.server) !== JSON.stringify(state.snap?.server ?? null);
  state.snap = snap;
  state.lang = snap.settings.language;
  if (snap.phase !== "installing") state.task = null;
  if (serverChanged && !snap.server) {
    state.news = [];
    state.branding = { backgrounds: [], accent: null };
    state.extrasFor = "";
    startArt();
  }
  if (langChanged) forceRender($("#view-play"));
  if (streamerChanged || serverChanged) {
    state.revealed.clear();
    refreshSecrets();
  }
  // The game runs next to the launcher: the background holds still meanwhile (styles.css).
  $("#hero").classList.toggle("game-running", snap.running.game);
  renderAll();
  void loadExtras(snap);
}

function initChrome(): void {
  const min = $("#win-min");
  const max = $("#win-max");
  const close = $("#win-close");
  min.appendChild(icon("minimize"));
  max.appendChild(icon("maximize"));
  close.appendChild(icon("close"));
  min.addEventListener("click", () => void api.minimize());
  max.addEventListener("click", () => void api.toggleMaximize());
  close.addEventListener("click", () => void api.close());
  api.onWindowState((maximized) => {
    state.maximized = maximized;
    max.replaceChildren(icon(maximized ? "restore" : "maximize"));
    max.setAttribute("aria-label", t(maximized ? "window_restore" : "window_maximize"));
  });
  for (const b of Array.from(document.querySelectorAll<HTMLButtonElement>("#lang-switch .lang-btn"))) {
    b.addEventListener("click", () => void api.setSettings({ language: LANGUAGES.find((l) => l === b.dataset.lang) ?? "en" }));
  }
  const credits = $("#credits-btn");
  credits.prepend(icon("heart"));
  credits.addEventListener("click", () => setView("credits"));
  const github = $("#github-btn");
  github.appendChild(githubMark());
  github.addEventListener("click", () => open("project_source"));
  document.addEventListener("visibilitychange", () => void api.setStatusPolling(!document.hidden));
  window.setInterval(() => {
    renderPanel();
    if (state.view === "server") renderServer();
  }, 30000);
}

async function main(): Promise<void> {
  initChrome();
  buildScene();
  applyStaticI18n();
  renderAll();
  api.onSnapshot(onSnapshot);
  api.onProgress((p) => {
    state.task = p;
    renderActionBar();
  });
  api.onHostKit((k) => {
    state.host = k;
    if (state.view === "host") renderHost();
  });
  api.onHostServer((sv) => {
    const changed = state.hostServer?.state !== sv.state || state.hostServer?.message !== sv.message || JSON.stringify(state.hostServer?.invite) !== JSON.stringify(sv.invite);
    state.hostServer = sv;
    if (state.view !== "host") return;
    if (changed) renderHost();
    else renderHostConsole();
  });
  api.onInviteLink(() => void checkInviteLink());
  onSnapshot(await api.getSnapshot());
  void api.setStatusPolling(!document.hidden);
  await checkInviteLink();
}

void main();
