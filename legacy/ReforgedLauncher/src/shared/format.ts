// Number formatting for the UI (bytes, speed, time left, running time), in English and Brazilian
// Portuguese.
import type { Language } from "./types";

const UNITS = ["B", "KB", "MB", "GB", "TB"];

function decimal(text: string, lang: Language): string {
  return lang === "en" ? text : text.replace(".", ",");
}

export function formatBytes(bytes: number, lang: Language = "en"): string {
  if (!Number.isFinite(bytes) || bytes < 0) bytes = 0;
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < UNITS.length - 1) {
    value /= 1000;
    unit++;
  }
  const digits = unit === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${decimal(value.toFixed(digits), lang)} ${UNITS[unit]}`;
}

export function formatSpeed(bytesPerSecond: number, lang: Language = "en"): string {
  return `${formatBytes(bytesPerSecond, lang)}/s`;
}

// 277200 -> "3 d 5 h", 3725 -> "1 h 2 min", 95 -> "1 min 35 s", 12 -> "12 s"
export function formatDuration(seconds: number | null, lang: Language = "en"): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return "–";
  void lang; // the same short units read naturally in both languages
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h >= 24) return `${Math.floor(h / 24)} d ${h % 24} h`;
  if (h > 0) return `${h} h ${m} min`;
  if (m > 0) return `${m} min ${sec} s`;
  return `${sec} s`;
}

export function formatPercent(done: number, total: number): number {
  if (!(total > 0)) return 0;
  return Math.max(0, Math.min(100, Math.floor((done / total) * 1000) / 10));
}

// "running for" on instance cards: 42 min, 3 h 5 min, 2 d 4 h
export function formatRunningTime(startedAt: string | null, now: number, lang: Language = "en"): string {
  if (startedAt === null) return "–";
  const t = Date.parse(startedAt);
  if (!Number.isFinite(t)) return "–";
  const minutes = Math.max(0, Math.floor((now - t) / 60000));
  if (minutes < 1) return lang === "pt-BR" ? "acabou de começar" : "just started";
  if (minutes < 60) return `${minutes} min`;
  return formatDuration(minutes * 60, lang);
}

const MONTHS: Record<Language, string[]> = {
  en: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
  "pt-BR": ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"],
};

export function formatDate(iso: string | null, lang: Language = "en"): string {
  if (iso === null) return "";
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return "";
  const d = new Date(t);
  const names = MONTHS[lang] ?? MONTHS.en;
  return `${d.getDate()} ${names[d.getMonth()]} ${d.getFullYear()}`;
}

export function formatPercentText(value: number, lang: Language = "en"): string {
  return `${decimal(value.toFixed(value < 10 ? 1 : 0), lang)} %`;
}
