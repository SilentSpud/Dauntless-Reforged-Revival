// Throwaway self-signed certificates for the TLS tests, written to a temp directory. The key pair
// comes from Node's crypto (fast); node-forge builds and signs the certificate, the same way the
// gateway's make-cert tool does.

import { generateKeyPairSync, createHash } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import forge from "node-forge";

export interface TestCert {
  certPem: string;
  keyPem: string;
  fingerprint: string; // SHA-256 of the DER certificate, lower-case hex
  dir: string;
  cleanup(): void;
}

export function makeTestCert(commonName = "127.0.0.1"): TestCert {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const keyPem = privateKey.export({ type: "pkcs1", format: "pem" }).toString();
  const pubPem = publicKey.export({ type: "spki", format: "pem" }).toString();
  const cert = forge.pki.createCertificate();
  cert.publicKey = forge.pki.publicKeyFromPem(pubPem);
  cert.serialNumber = "01" + createHash("sha256").update(String(Math.random())).digest("hex").slice(0, 30);
  cert.validity.notBefore = new Date(Date.now() - 60_000);
  cert.validity.notAfter = new Date(Date.now() + 10 * 365 * 24 * 3600 * 1000);
  const attrs = [{ name: "commonName", value: commonName }];
  cert.setSubject(attrs);
  cert.setIssuer(attrs);
  cert.setExtensions([
    { name: "basicConstraints", cA: false },
    { name: "keyUsage", digitalSignature: true, keyEncipherment: true },
    { name: "extKeyUsage", serverAuth: true },
    { name: "subjectAltName", altNames: [{ type: 7, ip: "127.0.0.1" }, { type: 2, value: "localhost" }] },
  ]);
  cert.sign(forge.pki.privateKeyFromPem(keyPem), forge.md.sha256.create());
  const certPem = forge.pki.certificateToPem(cert);
  const der = Buffer.from(forge.asn1.toDer(forge.pki.certificateToAsn1(cert)).getBytes(), "binary");
  const fingerprint = createHash("sha256").update(der).digest("hex");
  const dir = mkdtempSync(path.join(tmpdir(), "rr-launcher-cert-"));
  writeFileSync(path.join(dir, "cert.pem"), certPem);
  writeFileSync(path.join(dir, "key.pem"), keyPem, { mode: 0o600 });
  return { certPem, keyPem, fingerprint, dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}
