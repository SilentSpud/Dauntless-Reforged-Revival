// Writes the expected answers in invite.json, status.json and username.json from the TypeScript the
// Rust code ports (legacy/ReforgedLauncher/src/shared), keeping each table's inputs. The tables began as
// Dauntless Revived's (legacy/UndauntedLauncher/test/cases at e1c5fec) with Reforged's scheme and key
// prefix put in. The one place the Rust answer is not the TypeScript's: a dauntless-revived:// invite,
// which Reforged's TypeScript reads as a 1.4.4 server and the Rust refuses as "revived" (RD5).
//
// Usage, from this folder, with the four TypeScript files compiled to CommonJS:
//   tsc --target es2022 --module commonjs --outDir <out> <legacy/ReforgedLauncher/src/shared>/{invite,status,username,text}.ts
//   node generate.js <out>
const fs = require("node:fs");
const path = require("node:path");

const compiled = path.resolve(process.argv[2] ?? "");
const { parseInvite } = require(path.join(compiled, "invite.js"));
const { parseServerStatus } = require(path.join(compiled, "status.js"));
const { checkUsername, extractAccountKey, isPlausibleAccountKey } = require(path.join(compiled, "username.js"));

const read = (name) => JSON.parse(fs.readFileSync(path.join(__dirname, name), "utf8"));
const write = (name, value) => fs.writeFileSync(path.join(__dirname, name), JSON.stringify(value, null, 2) + "\n");

const REVIVED = /^dauntless-revived:\/\/join\/?\?/i;
const invites = read("invite.json");
const accept = [];
const reject = [];
for (const { input } of [...invites.accept, ...invites.reject]) {
  const result = parseInvite(input);
  const text = input.trim();
  if (REVIVED.test(text) && !(result.ok === false && (result.error === "empty" || result.error === "too_long"))) {
    reject.push({ input, error: "revived" });
  } else if (result.ok) {
    const { game: _game, ...invite } = result.invite;
    accept.push({ input, invite });
  } else {
    reject.push({ input, error: result.error });
  }
}
write("invite.json", { accept, reject });

// "text" is what JSON.stringify gives the page, so the field order is checked too
write("status.json", read("status.json").map(({ input }) => {
  const expected = parseServerStatus(input);
  return { input, expected, text: JSON.stringify(expected) };
}));

const usernames = read("username.json");
write("username.json", {
  check: usernames.check.map(({ input }) => ({ input, result: checkUsername(input) })),
  plausibleKey: usernames.plausibleKey.map(({ input }) => ({ input, ok: isPlausibleAccountKey(input) })),
  extractKey: usernames.extractKey.map(({ input }) => ({ input, key: extractAccountKey(input) })),
});
console.log(`invite.json: ${accept.length} accepted, ${reject.length} refused`);
