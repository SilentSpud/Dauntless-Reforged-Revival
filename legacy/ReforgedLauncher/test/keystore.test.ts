// The key store: a public server's key belongs to one certificate fingerprint.

import { test, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { KeyStore, KeyStoreError, serverId, type Encryptor, type KeySlot } from "../src/main/keystore";

const enc: Encryptor = {
  isAvailable: () => true,
  encrypt: (plain) => Buffer.from("enc:" + Buffer.from(plain).toString("base64")),
  decrypt: (data) => {
    const s = data.toString();
    if (!s.startsWith("enc:")) throw new Error("not ours");
    return Buffer.from(s.slice(4), "base64").toString();
  },
};
const dirs: string[] = [];
function store(): { ks: KeyStore; dir: string } {
  const dir = mkdtempSync(path.join(tmpdir(), "dr-keys-"));
  dirs.push(dir);
  return { ks: new KeyStore(dir, enc), dir };
}
after(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});

const KEY = "RRK_" + "ab".repeat(24);
const FP_A = "a1".repeat(32);
const FP_B = "b2".repeat(32);
const pub = (fp: string | null): KeySlot => ({ host: "203.0.113.10", port: 443, mode: "public", fp });

test("a public key loads only for the certificate it was saved with", async () => {
  const { ks } = store();
  await ks.save(pub(FP_A), KEY);
  assert.equal(await ks.load(pub(FP_A)), KEY);
  assert.equal(await ks.has(pub(FP_A)), true);
  assert.equal(await ks.load(pub(FP_B)), null, "another certificate never gets the key");
  assert.equal(await ks.has(pub(FP_B)), false);
  assert.equal(await ks.load(pub(null)), null);
  assert.deepEqual(await ks.certificateOf(pub(FP_B)), { fp: FP_A });
});

test("rebind moves the key to a new certificate", async () => {
  const { ks } = store();
  await ks.save(pub(FP_A), KEY);
  assert.equal(await ks.rebind(pub(FP_A), FP_B), true);
  assert.equal(await ks.load(pub(FP_B)), KEY);
  assert.equal(await ks.load(pub(FP_A)), null);
  assert.equal(await store().ks.rebind(pub(FP_A), FP_B), false, "nothing to move");
  await assert.rejects(ks.rebind(pub(FP_B), "not-a-fingerprint"), KeyStoreError);
});

test("a public key cannot be saved without a fingerprint", async () => {
  const { ks } = store();
  await assert.rejects(ks.save(pub(null), KEY), KeyStoreError);
  await assert.rejects(ks.save(pub("ABC"), KEY), KeyStoreError);
});

test("private keys are not tied to a certificate; old key files still load for private servers only", async () => {
  const { ks, dir } = store();
  const priv: KeySlot = { host: "100.64.0.1", port: 61000, mode: "private", fp: null };
  await ks.save(priv, KEY);
  assert.equal(await ks.load(priv), KEY);
  // The first file format: the key alone.
  writeFileSync(path.join(dir, `${serverId("100.64.0.2", 61000, "private")}.key`), enc.encrypt(KEY));
  assert.equal(await ks.load({ host: "100.64.0.2", port: 61000, mode: "private", fp: null }), KEY);
  writeFileSync(path.join(dir, `${serverId("203.0.113.10", 443, "public")}.key`), enc.encrypt(KEY));
  assert.equal(await ks.load(pub(FP_A)), null, "an untied public key goes nowhere");
  assert.deepEqual(await ks.certificateOf(pub(FP_A)), { fp: null });
});

test("remove deletes the key whatever certificate it belongs to", async () => {
  const { ks } = store();
  await ks.save(pub(FP_A), KEY);
  await ks.remove(pub(FP_B));
  assert.equal(await ks.certificateOf(pub(FP_A)), null);
});

test("Documents recovery survives lost local key storage and keeps certificate binding", async () => {
  const { dir } = store();
  const recovery = path.join(dir, "Documents");
  const primary = path.join(dir, "local");
  const ks = new KeyStore(primary, enc, recovery);
  await ks.save(pub(FP_A), KEY);
  rmSync(primary, {recursive:true, force:true});
  const restored = new KeyStore(primary, enc, recovery);
  assert.equal(await restored.load(pub(FP_B)), null);
  assert.equal(await restored.load(pub(FP_A)), KEY);
  await restored.remove(pub(FP_A));
  assert.equal(await restored.load(pub(FP_A)), null);
});
