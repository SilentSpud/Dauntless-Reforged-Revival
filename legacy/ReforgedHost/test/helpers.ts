import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const DIRS: string[] = [];
process.once("exit", () => {
    for (const d of DIRS) fs.rmSync(d, { recursive: true, force: true });
});

export function tempDir(name: string): string {
    const d = fs.mkdtempSync(path.join(os.tmpdir(), `rr-host-${name}-`));
    DIRS.push(d);
    return d;
}

export function write(file: string, data: string | Buffer): void {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, data);
}
