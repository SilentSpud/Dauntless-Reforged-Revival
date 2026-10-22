// Makes a test launcher profile the way the Electron launcher (1.5.x) does, with its own keystore.ts and
// settings.ts and Electron's real safeStorage, so the Rust key store can be shown to read it back
// (MIGRATION_PLAN.md RA8; the Rust side is `an_electron_profile_reads_back` in platform/keystore/mod.rs).
// The keys are made up here; never point this at a real profile. See README.md.
//   npx electron . <empty profile folder>
const { app, safeStorage } = require("electron");
const { randomBytes } = require("node:crypto");
const { mkdirSync, readdirSync, writeFileSync, existsSync } = require("node:fs");
const path = require("node:path");

const profile = path.resolve(process.argv[process.argv.length - 1]);
if (existsSync(profile) && readdirSync(profile).length > 0) {
  console.error(`${profile} is not empty: give an empty folder`);
  process.exit(1);
}
const userData = path.join(profile, "User Data");
const documents = path.join(profile, "Documents");
mkdirSync(userData, { recursive: true });
app.setPath("userData", userData);
// As the Electron launcher: its name decides nothing on Windows (DPAPI), but the Linux keyring entry
app.setName("Dauntless Reforged Revival Launcher");

app.whenReady().then(async () => {
  try {
    const { KeyStore, serverId } = require("./build/main/keystore");
    const { SettingsStore } = require("./build/main/settings");
    if (!safeStorage.isEncryptionAvailable()) throw new Error("safeStorage is not available");
    const encryptor = {
      isAvailable: () => safeStorage.isEncryptionAvailable(),
      encrypt: (plain) => safeStorage.encryptString(plain),
      decrypt: (data) => safeStorage.decryptString(data),
    };
    const keysDir = path.join(userData, "keys");
    const recovery = path.join(documents, "Dauntless Reforged Revival", "Account Keys");
    const store = new KeyStore(keysDir, encryptor, recovery);
    const key = () => "RRK_" + randomBytes(24).toString("hex");
    const fp = () => randomBytes(32).toString("hex");
    const expected = [];
    const slot = (host, port, mode, fingerprint) => ({ host, port, mode, fp: fingerprint });

    // A public server's key, bound to its certificate
    const a = slot("play.example.org", 443, "public", fp());
    const ka = key();
    await store.save(a, ka);
    expected.push({ ...a, key: ka, certificate: a.fp, documents: true });
    // The same server asked for with another certificate gets nothing
    expected.push({ ...a, fp: fp(), key: null, certificate: a.fp, documents: true });
    // A private (Tailscale) server's key
    const b = slot("100.64.0.7", 61000, "private", null);
    const kb = key();
    await store.save(b, kb);
    expected.push({ ...b, key: kb, certificate: null, documents: true });
    // A key moved to a new certificate after the player confirmed
    const c = slot("hunts.example.net", 8443, "public", fp());
    const kc = key();
    await store.save(c, kc);
    const moved = fp();
    await store.rebind(c, moved);
    expected.push({ ...c, fp: moved, key: kc, certificate: moved, documents: true });
    expected.push({ ...c, key: null, certificate: moved, documents: true });
    // The first file format, a bare key: loads for a private server, never for a public one
    const d = slot("100.64.0.8", 61000, "private", null);
    const kd = key();
    writeFileSync(path.join(keysDir, `${serverId(d.host, d.port, d.mode)}.key`), encryptor.encrypt(kd));
    expected.push({ ...d, key: kd, certificate: null, documents: false });
    const e = slot("old-cert.example.org", 443, "public", fp());
    writeFileSync(path.join(keysDir, `${serverId(e.host, e.port, e.mode)}.key`), encryptor.encrypt(key()));
    expected.push({ ...e, key: null, certificate: null, documents: false });
    // A Dauntless Revived (1.4.4) server: its key stays and still reads
    const f = slot("revived.example.org", 443, "public", fp());
    const kf = key();
    await store.save(f, kf);
    expected.push({ ...f, key: kf, certificate: f.fp, documents: true });

    // settings.json with the 1.4.4 server current and a Reforged one saved
    const settings = new SettingsStore(userData, "en", process.platform);
    const server = (s, game, name) => ({ game, mode: s.mode, host: s.host, port: s.port, name, share: null, fp: s.fp, code: null });
    await settings.update((s) => {
      s.server = server(f, "revived", "Old hunts");
      s.savedServers = [server(f, "revived", "Old hunts"), server(a, "reforged", "Hunts")];
      s.revivedRoot = process.platform === "win32" ? "D:\\Dauntless144" : "/opt/dauntless144";
      s.usernames = { [serverId(a.host, a.port, a.mode)]: "Slayer_01" };
    });

    writeFileSync(path.join(profile, "expected.json"), JSON.stringify({ platform: process.platform, slots: expected }, null, 2));
    console.log(`profile written: ${profile} (${expected.length} slots; keys in expected.json only)`);
    app.exit(0);
  } catch (error) {
    console.error(error);
    app.exit(1);
  }
});
