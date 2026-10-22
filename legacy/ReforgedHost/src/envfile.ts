// .env files: KEY=value lines, as Node's --env-file reads them. Values are written unquoted when
// they are plain and double-quoted otherwise.

export type EnvMap = Record<string, string>;

export function parseEnv(text: string): EnvMap {
    const out: EnvMap = {};
    for (const raw of text.replace(/^﻿/, "").split(/\r?\n/)) {
        const line = raw.trim();
        if (line === "" || line.startsWith("#")) continue;
        const eq = line.indexOf("=");
        if (eq <= 0) continue;
        const key = line.slice(0, eq).trim().replace(/^export\s+/, "");
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;
        let value = line.slice(eq + 1).trim();
        if (value.length >= 2 && (value[0] === '"' || value[0] === "'") && value[value.length - 1] === value[0]) {
            const quote = value[0];
            value = value.slice(1, -1);
            if (quote === '"') value = value.replace(/\\n/g, "\n").replace(/\\"/g, '"').replace(/\\\\/g, "\\");
        }
        out[key] = value;
    }
    return out;
}

function formatValue(value: string): string {
    if (/^[A-Za-z0-9_./:@,+=%-]*$/.test(value)) return value;
    if (value.includes("\n") || value.includes("\r")) throw new Error("multi-line values are not supported in .env files");
    return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

// Lines to write: optional comments (# ...) and the values in the given order.
export function formatEnv(header: string[], entries: [string, string][]): string {
    const lines = header.map((h) => (h === "" ? "#" : `# ${h}`));
    if (lines.length) lines.push("");
    for (const [key, value] of entries) {
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) throw new Error(`bad env key ${key}`);
        lines.push(`${key}=${formatValue(value)}`);
    }
    return lines.join("\n") + "\n";
}
