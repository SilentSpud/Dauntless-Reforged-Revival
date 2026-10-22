import { test } from "node:test";
import assert from "node:assert/strict";
import { formatInvite, isPrivateModeHost, isTailscaleIPv4, isValidFingerprint, isValidHost, parseInvite, type Invite } from "../src/shared/invite";
import { RLO } from "../src/shared/text";

const FP = "3f".repeat(32);
const good = "reforged-revival://join?v=1&host=100.101.102.103&port=61000&code=ABCD-1234&name=Alex%27s%20Ramsgate";
const goodV2 = `reforged-revival://join?v=2&mode=public&host=203.0.113.10&port=443&fp=${FP}&code=ABCD-EFGH-JKLM&name=Friday%20Hunts`;

// ------------------------------------------------------------------ v1 (private, Tailscale)

test("v1: parses a valid invite", () => {
  const r = parseInvite(good);
  assert.equal(r.ok, true);
  if (!r.ok) return;
  assert.deepEqual(r.invite, { game: "reforged", mode: "private", host: "100.101.102.103", port: 61000, code: "ABCD-1234", name: "Alex's Ramsgate", share: null, fp: null });
});

test("v1: accepts surrounding whitespace, a MagicDNS name, a share link and an upper-case scheme", () => {
  const share = encodeURIComponent("https://login.tailscale.com/admin/invite/abcDEF123");
  const r = parseInvite(`  REFORGED-REVIVAL://join/?v=1&host=HostPC.tail1234.ts.net&port=61000&code=abcd&name=Friends+Night&share=${share}\n`);
  assert.equal(r.ok, true);
  if (!r.ok) return;
  assert.equal(r.invite.mode, "private");
  assert.equal(r.invite.host, "hostpc.tail1234.ts.net");
  assert.equal(r.invite.name, "Friends Night");
  assert.equal(r.invite.share, "https://login.tailscale.com/admin/invite/abcDEF123");
});

test("v1: round-trips through formatInvite", () => {
  const inv: Invite = { game: "reforged", mode: "private", host: "100.64.0.1", port: 61000, code: "Code-42", name: "Ember & Aether", share: "https://login.tailscale.com/x", fp: null };
  const r = parseInvite(formatInvite(inv));
  assert.equal(r.ok, true);
  if (r.ok) assert.deepEqual(r.invite, inv);
});

// ------------------------------------------------------------------ v2 (public, TLS gateway)

test("v2: parses a valid public invite", () => {
  const r = parseInvite(goodV2);
  assert.equal(r.ok, true);
  if (!r.ok) return;
  assert.deepEqual(r.invite, { game: "reforged", mode: "public", host: "203.0.113.10", port: 443, code: "ABCD-EFGH-JKLM", name: "Friday Hunts", share: null, fp: FP });
});

test("v2: the port defaults to 443, a DNS name and any parameter order work", () => {
  const r = parseInvite(`reforged-revival://join?name=Hunts&code=abcd&fp=${FP}&host=Play.Example.ORG&mode=public&v=2`);
  assert.equal(r.ok, true);
  if (!r.ok) return;
  assert.equal(r.invite.port, 443);
  assert.equal(r.invite.host, "play.example.org");
  assert.equal(r.invite.mode, "public");
});

test("v2: a custom gateway port", () => {
  const r = parseInvite(`reforged-revival://join?v=2&mode=public&host=198.51.100.7&port=8443&fp=${FP}&code=abcd&name=x`);
  assert.equal(r.ok, true);
  if (r.ok) assert.equal(r.invite.port, 8443);
});

test("v2: round-trips through formatInvite", () => {
  const inv: Invite = { game: "reforged", mode: "public", host: "play.example.org", port: 8443, code: "XXXX-YYYY", name: "Aether & Ember", share: null, fp: FP };
  const r = parseInvite(formatInvite(inv));
  assert.equal(r.ok, true);
  if (r.ok) assert.deepEqual(r.invite, inv);
  assert.throws(() => formatInvite({ ...inv, fp: null }));
});

const v2 = (params: string) => `reforged-revival://join?${params}`;
const base2 = { v: "2", mode: "public", host: "203.0.113.10", port: "443", fp: FP, code: "abcd", name: "x" };
const build2 = (over: Record<string, string | null>) =>
  v2(
    Object.entries({ ...base2, ...over })
      .filter(([, v]) => v !== null)
      .map(([k, v]) => `${k}=${v}`)
      .join("&"),
  );

// ------------------------------------------------------------------ malformed

const bad: [string, string][] = [
  ["", "empty"],
  ["   ", "empty"],
  ["hello", "not_invite"],
  ["https://example.com/join?v=1", "not_invite"],
  ["reforged-revival://register?v=1&host=1.2.3.4&port=1&code=abcd&name=x", "not_invite"],
  ["reforged-revival:join?v=1&host=1.2.3.4&port=1&code=abcd&name=x", "not_invite"],
  ["reforged-revival://join?v=3&host=100.64.0.1&port=61000&code=abcd&name=x", "version"],
  ["reforged-revival://join?v=01&host=100.64.0.1&port=61000&code=abcd&name=x", "version"],
  ["reforged-revival://join?host=100.64.0.1&port=61000&code=abcd&name=x", "version"],
  ["reforged-revival://join?v=1&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=100.64.0.1&code=abcd&name=x", "port"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&name=x", "code"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd", "name"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x&extra=1", "params"],
  ["reforged-revival://join?v=1&v=1&host=100.64.0.1&port=61000&code=abcd&name=x", "params"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x#frag", "params"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=%E0%A4%A", "params"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x&", "params"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&=x", "params"],
  // v1 does not take the v2 fields, and v2 does not take share
  [`reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x&fp=${FP}`, "params"],
  ["reforged-revival://join?v=1&mode=public&host=100.64.0.1&port=61000&code=abcd&name=x", "params"],
  ["reforged-revival://join?v=1&host=256.1.1.1&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=1.2.3&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=01.2.3.4&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=evil.com%2F%40x&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=user%40host&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=host.123&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=0x7f.1&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=-bad-.ts.net&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=a..b&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=%5B%3A%3A1%5D&port=61000&code=abcd&name=x", "host"],
  // v1 is plain HTTP: only Tailscale addresses, *.ts.net names and loopback
  ["reforged-revival://join?v=1&host=203.0.113.9&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=evil.example.com&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=192.168.1.10&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=100.128.0.1&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=ts.net&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=hostpc.ts.net.evil.com&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=hostpc&port=61000&code=abcd&name=x", "host"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=0&code=abcd&name=x", "port"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=65536&code=abcd&name=x", "port"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=061000&code=abcd&name=x", "port"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000x&code=abcd&name=x", "port"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abc&name=x", "code"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=ab%20cd&name=x", "code"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=" + "a".repeat(65) + "&name=x", "code"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=%20%20", "name"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=a%0Ab", "name"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=" + encodeURIComponent(RLO) + "evil", "name"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=" + "n".repeat(65), "name"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x&share=http%3A%2F%2Flogin.tailscale.com%2Fx", "share"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x&share=https%3A%2F%2Fevil.com%2F", "share"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x&share=https%3A%2F%2Flogin.tailscale.com.evil.com%2F", "share"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x&share=https%3A%2F%2Fa%40login.tailscale.com%2F", "share"],
  ["reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x&share=https%3A%2F%2Flogin.tailscale.com%3A8443%2F", "share"],
  ["reforged-revival://join?" + "x".repeat(3000), "too_long"],
  // v2
  [build2({ mode: null }), "mode"],
  [build2({ mode: "private" }), "mode"],
  [build2({ mode: "PUBLIC" }), "mode"],
  [build2({ mode: "" }), "mode"],
  [build2({ fp: null }), "fp"],
  [build2({ fp: FP.toUpperCase() }), "fp"],
  [build2({ fp: FP.slice(0, 63) }), "fp"],
  [build2({ fp: FP + "a" }), "fp"],
  [build2({ fp: FP.slice(0, 62) + "zz" }), "fp"],
  [build2({ fp: FP.replace(/(..)/g, "$1:").slice(0, -1) }), "fp"],
  [build2({ fp: "sha256%2F" + FP }), "fp"],
  [build2({ host: null }), "host"],
  [build2({ host: "203.0.113.999" }), "host"],
  [build2({ host: "exa_mple.com" }), "host"],
  [build2({ host: "https%3A%2F%2Fexample.com" }), "host"],
  [build2({ port: "0" }), "port"],
  [build2({ port: "443.0" }), "port"],
  [build2({ port: "" }), "port"],
  [build2({ code: null }), "code"],
  [build2({ code: "a+b+c+d" }), "code"],
  [build2({ name: null }), "name"],
  [build2({ name: "%09tab" }), "name"],
  [build2({ share: encodeURIComponent("https://login.tailscale.com/x") }), "params"],
  [build2({ extra: "1" }), "params"],
  [build2({}) + `&fp=${FP}`, "params"],
];

for (const [input, error] of bad) {
  test(`rejects (${error}): ${input.slice(0, 100)}`, () => {
    const r = parseInvite(input);
    assert.equal(r.ok, false);
    if (!r.ok) assert.equal(r.error, error);
  });
}

test("rejects non-strings", () => {
  for (const v of [undefined, null, 42, {}, ["reforged-revival://join?v=1"]]) {
    assert.equal(parseInvite(v).ok, false);
  }
});

test("v1: loopback is allowed (the host's own PC, tests)", () => {
  for (const host of ["127.0.0.1", "localhost"]) {
    const r = parseInvite(`reforged-revival://join?v=1&host=${host}&port=61000&code=abcd&name=x`);
    assert.equal(r.ok, true, host);
  }
});

test("v2 still takes any public address or DNS name", () => {
  for (const host of ["203.0.113.9", "evil.example.com", "hostpc.tail1234.ts.net"]) {
    assert.equal(parseInvite(build2({ host })).ok, true, host);
  }
});

test("private-mode host rule", () => {
  assert.equal(isPrivateModeHost("100.64.0.1"), true);
  assert.equal(isPrivateModeHost("my-pc.tail-scale.ts.net"), true);
  assert.equal(isPrivateModeHost("127.0.0.1"), true);
  assert.equal(isPrivateModeHost("localhost"), true);
  assert.equal(isPrivateModeHost("203.0.113.9"), false);
  assert.equal(isPrivateModeHost("10.0.0.5"), false);
  assert.equal(isPrivateModeHost("example.com"), false);
  assert.equal(isPrivateModeHost("ts.net"), false);
});

test("host and fingerprint rules", () => {
  assert.equal(isValidHost("100.64.0.1"), true);
  assert.equal(isValidHost("localhost"), true);
  assert.equal(isValidHost("my-pc.tail-scale.ts.net"), true);
  assert.equal(isValidHost("MyPC"), false); // upper case is lowered by the parser, not here
  assert.equal(isValidHost("a_b"), false);
  assert.equal(isTailscaleIPv4("100.64.0.1"), true);
  assert.equal(isTailscaleIPv4("100.127.255.255"), true);
  assert.equal(isTailscaleIPv4("100.128.0.1"), false);
  assert.equal(isTailscaleIPv4("192.168.1.1"), false);
  assert.equal(isValidFingerprint(FP), true);
  assert.equal(isValidFingerprint(FP.toUpperCase()), false);
  assert.equal(isValidFingerprint(""), false);
  assert.equal(isValidFingerprint(null), false);
});

test("Dauntless Revived invites are read too, as servers for Dauntless 1.4.4", () => {
  const priv = parseInvite("dauntless-revived://join?v=1&host=100.88.12.40&port=61000&code=ABCD-1234&name=Revived%20Fridays");
  assert.equal(priv.ok, true);
  if (priv.ok) {
    assert.equal(priv.invite.game, "revived");
    assert.equal(priv.invite.mode, "private");
    assert.equal(priv.invite.port, 61000);
  }
  const fp = "ab".repeat(32);
  const pub = parseInvite(`dauntless-revived://join?v=2&mode=public&host=revived.example.net&port=443&fp=${fp}&code=XYZ-99&name=Open`);
  assert.equal(pub.ok && pub.invite.game, "revived");
  const ours = parseInvite("reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=Code-42&name=Ours");
  assert.equal(ours.ok && ours.invite.game, "reforged");
  // The same strict rules: plain HTTP only into the tailnet.
  assert.deepEqual(parseInvite("dauntless-revived://join?v=1&host=203.0.113.7&port=61000&code=ABCD-1234&name=X"), { ok: false, error: "host" });
  const inv: Invite = { game: "revived", mode: "private", host: "100.88.12.40", port: 61000, code: "ABCD-1234", name: "Revived Fridays", share: null, fp: null };
  assert.ok(formatInvite(inv).startsWith("dauntless-revived://join?"));
  assert.deepEqual(parseInvite(formatInvite(inv)), { ok: true, invite: inv });
});
