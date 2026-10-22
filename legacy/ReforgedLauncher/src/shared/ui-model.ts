// What the UI shows for a given Snapshot. Pure functions, so the renderer's decisions can be
// tested without a DOM.

import type { StringKey } from "./i18n";
import type { GraphicsPreset, Language, Phase, Snapshot, TaskProgress } from "./types";
import type { InstanceKind, PlayerWhere } from "./status";
import { formatBytes, formatDuration, formatPercent, formatSpeed } from "./format";

export type PrimaryAction = "join" | "retry" | "new_invite" | "register" | "locate" | "verify" | "trust" | "install_runtime" | "revived_install" | "play" | "cancel" | "none";

export interface PrimaryButton {
  labelKey: StringKey;
  action: PrimaryAction;
  disabled: boolean;
}

export interface FormState {
  inviteValid: boolean;
  usernameValid: boolean;
}

export function primaryButton(s: Snapshot, form: FormState): PrimaryButton {
  const busy = s.busy;
  switch (s.phase) {
    case "loading":
      return { labelKey: "btn_wait", action: "none", disabled: true };
    case "join":
      return { labelKey: busy ? "btn_wait" : "btn_join", action: "join", disabled: busy || !form.inviteValid };
    case "connect":
      if (s.connect.checking) return { labelKey: "btn_wait", action: "none", disabled: true };
      // A server with the wrong certificate was refused on purpose: retrying does not help, a new
      // invite from the host does.
      if (s.connect.problem === "cert_mismatch") return { labelKey: "btn_new_invite", action: "new_invite", disabled: busy };
      return { labelKey: "btn_retry", action: "retry", disabled: busy };
    case "register":
      return {
        labelKey: busy ? "btn_wait" : "btn_register",
        action: "register",
        disabled: busy || !form.usernameValid || s.status?.registration === "NONE",
      };
    case "locate":
      // Dauntless 1.4.4 (a Dauntless Revived server): download it, or check (and repair) what is there.
      if (s.revived) return { labelKey: s.revived.problem === "not_set" ? "btn_download" : "btn_check", action: "revived_install", disabled: busy };
      if (s.game.problem === "unverified") return { labelKey: "btn_verify", action: "verify", disabled: busy };
      return { labelKey: "btn_locate", action: "locate", disabled: busy };
    case "runtime":
      switch (s.runtime.state) {
        case "untrusted":
          return { labelKey: "btn_trust", action: "trust", disabled: busy };
        case "missing":
        case "outdated":
          return { labelKey: s.runtime.state === "missing" ? "btn_install" : "btn_update", action: "install_runtime", disabled: busy };
        default:
          // "unavailable": nothing to install until the host publishes a runtime.
          return { labelKey: "btn_waiting_host", action: "none", disabled: true };
      }
    case "installing":
      return { labelKey: "btn_cancel", action: "cancel", disabled: false };
    case "ready":
      return { labelKey: busy ? "btn_wait" : "btn_play", action: "play", disabled: busy };
    case "running":
      return { labelKey: "btn_running", action: "none", disabled: true };
  }
}

// 0 Join, 1 Register, 2 Game, 3 Runtime, 4 Play
export function stepIndex(phase: Phase): number {
  switch (phase) {
    case "loading":
    case "join":
    case "connect":
      return 0;
    case "register":
      return 1;
    case "locate":
      return 2;
    case "runtime":
    case "installing":
      return 3;
    case "ready":
    case "running":
      return 4;
  }
}

export const STEP_KEYS: readonly StringKey[] = ["step_join", "step_register", "step_game", "step_runtime", "step_play"];

export interface TaskView {
  titleKey: StringKey;
  percent: number;
  bytesText: string | null; // "1.2 MB / 3.4 MB"
  speedText: string | null;
  etaText: string | null;
  filesText: string;
}

export function taskView(t: TaskProgress, lang: Language): TaskView {
  const titleKey: StringKey = t.kind === "runtime" ? "task_runtime" : t.kind === "download" ? "task_download" : t.kind === "dlls" ? "task_dlls" : "task_verify";
  const moving = t.kind === "runtime" || t.kind === "download";
  return {
    titleKey,
    percent: formatPercent(t.doneBytes, t.totalBytes),
    // The quick 1.4.4 check counts files, not bytes.
    bytesText: t.kind === "dlls" || (t.kind === "verify" && t.totalBytes === t.filesTotal) ? null : `${formatBytes(t.doneBytes, lang)} / ${formatBytes(t.totalBytes, lang)}`,
    speedText: moving && t.bytesPerSecond > 0 ? formatSpeed(t.bytesPerSecond, lang) : null,
    etaText: moving && t.etaSeconds !== null ? formatDuration(t.etaSeconds, lang) : null,
    filesText: `${t.filesDone}/${t.filesTotal}`,
  };
}

export function whereKey(w: PlayerWhere): StringKey {
  return `where_${w}` as StringKey;
}

export function kindKey(k: InstanceKind): StringKey {
  return `kind_${k}` as StringKey;
}

export function graphicsKey(g: GraphicsPreset): StringKey {
  return g < 0 ? "gfx_menu" : (`gfx_${g}` as StringKey);
}

// A short display form of a SHA-256: the first 16 hex digits in groups of four.
export function shortHash(hex: string): string {
  return `${(hex.slice(0, 16).match(/.{1,4}/g) ?? []).join(" ")}…`;
}

// How the Host page rates each step.
export type StepState = "done" | "todo" | "warn";

export interface HostSteps {
  workspace: StepState;
  sdk: StepState;
  build: StepState;
  gameData: StepState;
  publish: StepState;
}

export function hostSteps(k: {
  workspaceOk: boolean;
  upstream: { present: boolean; patched: boolean };
  sdk: { present: boolean; functionFiles: number };
  builds: { runtime: boolean; loader: boolean; exporter: boolean; published: boolean };
  gameData: { backend: "real" | "synthetic" | "missing"; director: "real" | "synthetic" | "missing" };
}): HostSteps {
  const data = [k.gameData.backend, k.gameData.director];
  return {
    workspace: k.workspaceOk && k.upstream.patched ? "done" : k.workspaceOk ? "warn" : "todo",
    sdk: k.sdk.present && k.sdk.functionFiles > 0 ? "done" : "todo",
    build: k.builds.runtime && k.builds.loader ? "done" : "todo",
    gameData: data.every((d) => d === "real") ? "done" : data.every((d) => d !== "missing") ? "warn" : "todo",
    publish: k.builds.published ? "done" : "todo",
  };
}
