// Compiles the unit tests with tsc and runs them with node:test.
//   npm test            all tests
//   npm test -- invite  only test files whose name contains "invite"
import { spawnSync } from "node:child_process";
import { readdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const out = path.join(root, ".test-build");
rmSync(out, { recursive: true, force: true });

const require = createRequire(import.meta.url);
const tsc = require.resolve("typescript/bin/tsc");
const build = spawnSync(process.execPath, [tsc, "-p", path.join(root, "tsconfig.test.json")], { stdio: "inherit", cwd: root });
if (build.status !== 0) process.exit(build.status ?? 1);

const filter = process.argv[2] ?? "";
const dir = path.join(out, "test");
const files = readdirSync(dir)
  .filter((f) => f.endsWith(".test.js") && f.includes(filter))
  .map((f) => path.join(dir, f));
if (files.length === 0) {
  console.error("no test files matched");
  process.exit(1);
}
const run = spawnSync(process.execPath, ["--test", "--test-reporter=spec", "--test-concurrency=1", ...files], { stdio: "inherit", cwd: root });
process.exit(run.status ?? 1);
