// Records the runtime list body legacy/ReforgedContent serves (RuntimeStore.get().body) for two
// runtime.json files written the way a host might: keys out of order, no build.name, files reversed.
// Usage: node record.js <legacy/ReforgedContent compiled with its own tsc (npm ci, npx tsc): its dist folder> <this folder>
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { RuntimeStore } = require(path.resolve(process.argv[2], "runtime.js"));
const out = process.argv[3];

const cases = {
  "runtime-manifest.json": (files) => ({
    files: [files.winmm, files.mystic],
    build: { changelist: 651234, name: "ignored by the server" },
    version: "20261014-1a2b3c4d",
  }),
  "runtime-manifest-defaults.json": (files) => ({
    version: "  1.0+rr_2  ",
    files: [{ sha256: files.mystic.sha256, size: files.mystic.size, name: files.mystic.name }, files.winmm],
  }),
};

for (const [name, make] of Object.entries(cases)) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "rr-runtime-"));
  const files = {};
  for (const [key, file, text] of [["mystic", "MysticParadox.dll", "mystic paradox\n"], ["winmm", "winmm.dll", "winmm loader\n"]]) {
    fs.writeFileSync(path.join(dir, file), text);
    const sha256 = require("node:crypto").createHash("sha256").update(text).digest("hex");
    files[key] = { name: file, size: Buffer.byteLength(text), sha256 };
  }
  const input = JSON.stringify(make(files), null, 2);
  fs.writeFileSync(path.join(dir, "runtime.json"), input);
  fs.writeFileSync(path.join(out, name.replace(".json", ".input.json")), input);
  const status = new RuntimeStore(dir).get();
  if (status.status !== "ok") throw new Error(`${name}: ${status.reason}`);
  fs.writeFileSync(path.join(out, name), status.body);
  console.log(name, status.body);
  fs.rmSync(dir, { recursive: true, force: true });
}
