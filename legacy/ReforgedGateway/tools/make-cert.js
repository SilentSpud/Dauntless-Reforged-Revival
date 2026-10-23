#!/usr/bin/env node
// Makes the gateway's self-signed TLS certificate: RSA-2048, SHA-256, valid 10 years, with the
// server's public IP and/or DNS name as subject alternative names. Prints the SHA-256 fingerprint
// of the certificate (DER), which goes into every v2 invite as fp=. Only writes the two PEM
// files; never touches any certificate store.
//
//   node tools/make-cert.js --host 203.0.113.7 [--host play.example.org] --out C:\ReforgedServer\tls
//   node tools/make-cert.js --host 203.0.113.7 --cert gw-cert.pem --key gw-key.pem [--force] [--json]
//   node tools/make-cert.js --fingerprint C:\ReforgedServer\tls\gateway-cert.pem
//
// Keep the key file private (a folder only Administrators and the gateway's account can read).
// A new certificate means a new fingerprint: invites made before it stop working.
"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const net = require("node:net");
const path = require("node:path");
const forge = require("node-forge");

const VALID_YEARS = 10;

function Usage(Message) {
    if (Message) {
        process.stderr.write(`make-cert: ${Message}\n`);
    }
    process.stderr.write("usage: node tools/make-cert.js --host <public IP or DNS name> [--host ...] (--out <dir> | --cert <file> --key <file>) [--force] [--json]\n");
    process.stderr.write("       node tools/make-cert.js --fingerprint <cert.pem> [--json]\n");
    process.exit(2);
}

function ParseArgs(Argv) {
    const Out = { hosts: [], out: undefined, cert: undefined, key: undefined, force: false, json: false, fingerprint: undefined };
    for (let Index = 0; Index < Argv.length; Index++) {
        const Arg = Argv[Index];
        const Next = () => {
            const Value = Argv[++Index];
            if (Value === undefined || Value.startsWith("--")) {
                Usage(`${Arg} needs a value`);
            }
            return Value;
        };
        switch (Arg) {
            case "--host": Out.hosts.push(Next()); break;
            case "--out": Out.out = Next(); break;
            case "--cert": Out.cert = Next(); break;
            case "--key": Out.key = Next(); break;
            case "--fingerprint": Out.fingerprint = Next(); break;
            case "--force": Out.force = true; break;
            case "--json": Out.json = true; break;
            case "--help": case "-h": Usage(); break;
            default: Usage(`unknown argument ${JSON.stringify(Arg)}`);
        }
    }
    return Out;
}

// A DNS name the way a certificate may carry it: letters, digits and hyphens in dot-separated
// labels, no wildcard, no trailing dot.
function IsDnsName(Name) {
    if (Name.length > 253 || !/[a-z]/i.test(Name)) {
        return false;
    }
    return Name.split(".").every((Label) => /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/i.test(Label));
}

// SubjectAltName entries: IP addresses as type 7, names as type 2.
function AltNames(Hosts) {
    if (Hosts.length === 0) {
        Usage("give at least one --host (the server's public IP address or DNS name)");
    }
    const Seen = new Set();
    const Names = [];
    for (const Raw of Hosts) {
        const Host = Raw.trim().toLowerCase();
        if (Seen.has(Host)) {
            continue;
        }
        Seen.add(Host);
        if (net.isIP(Host) !== 0) {
            if (Host.includes("%")) {
                Usage(`${Raw}: no zone ids`);
            }
            Names.push({ type: 7, ip: Host });
        }
        else if (IsDnsName(Host)) {
            Names.push({ type: 2, value: Host });
        }
        else {
            Usage(`${JSON.stringify(Raw)} is neither an IP address nor a DNS name`);
        }
    }
    return Names;
}

function Fingerprint(CertPem) {
    const Cert = new crypto.X509Certificate(CertPem);
    return {
        fingerprint: crypto.createHash("sha256").update(Cert.raw).digest("hex"),
        notBefore: new Date(Cert.validFrom).toISOString(),
        notAfter: new Date(Cert.validTo).toISOString(),
        names: Cert.subjectAltName || "",
    };
}

function MakeCertificate(Names) {
    // Uses Node's own RSA key generation under the hood (fast); the signing is node-forge.
    const Keys = forge.pki.rsa.generateKeyPair({ bits: 2048, e: 0x10001 });
    const Cert = forge.pki.createCertificate();
    Cert.publicKey = Keys.publicKey;

    // 16 random bytes, positive.
    const Serial = crypto.randomBytes(16);
    Serial[0] &= 0x7f;
    Serial[0] |= 0x01;
    Cert.serialNumber = Serial.toString("hex");

    // A day of slack for clocks that run behind.
    const NotBefore = new Date(Date.now() - 24 * 3600 * 1000);
    NotBefore.setUTCMilliseconds(0);
    const NotAfter = new Date(NotBefore.getTime());
    NotAfter.setUTCFullYear(NotAfter.getUTCFullYear() + VALID_YEARS);
    Cert.validity.notBefore = NotBefore;
    Cert.validity.notAfter = NotAfter;

    const First = Names[0].type === 7 ? Names[0].ip : Names[0].value;
    const Subject = [
        { name: "commonName", value: First },
        { name: "organizationName", value: "Dauntless Reforged Revival server" },
    ];
    Cert.setSubject(Subject);
    Cert.setIssuer(Subject);
    Cert.setExtensions([
        { name: "basicConstraints", cA: false, critical: true },
        { name: "keyUsage", critical: true, digitalSignature: true, keyEncipherment: true },
        { name: "extKeyUsage", serverAuth: true },
        { name: "subjectAltName", altNames: Names },
        { name: "subjectKeyIdentifier" },
    ]);
    Cert.sign(Keys.privateKey, forge.md.sha256.create());

    const CertPem = forge.pki.certificateToPem(Cert);
    // PKCS#8, the format Node and most tools expect today.
    const KeyPem = crypto.createPrivateKey(forge.pki.privateKeyToPem(Keys.privateKey)).export({ type: "pkcs8", format: "pem" });

    // Sanity: the key belongs to the certificate and the certificate parses.
    const Parsed = new crypto.X509Certificate(CertPem);
    if (!Parsed.checkPrivateKey(crypto.createPrivateKey(KeyPem))) {
        throw new Error("generated key does not match the certificate");
    }
    return { certPem: CertPem, keyPem: KeyPem };
}

function Main() {
    const Args = ParseArgs(process.argv.slice(2));

    if (Args.fingerprint !== undefined) {
        let Pem;
        try {
            Pem = fs.readFileSync(Args.fingerprint);
        }
        catch (error) {
            Usage(`cannot read ${Args.fingerprint}: ${error.message}`);
        }
        const Info = Fingerprint(Pem);
        if (Args.json) {
            process.stdout.write(JSON.stringify({ cert: path.resolve(Args.fingerprint), ...Info }) + "\n");
        }
        else {
            process.stdout.write(`Names:       ${Info.names}\nValid:       ${Info.notBefore} to ${Info.notAfter}\nFingerprint: ${Info.fingerprint}\n`);
        }
        return;
    }

    const Names = AltNames(Args.hosts);

    let CertFile;
    let KeyFile;
    if (Args.out !== undefined) {
        if (Args.cert !== undefined || Args.key !== undefined) {
            Usage("use either --out or --cert/--key");
        }
        CertFile = path.resolve(Args.out, "gateway-cert.pem");
        KeyFile = path.resolve(Args.out, "gateway-key.pem");
    }
    else if (Args.cert !== undefined && Args.key !== undefined) {
        CertFile = path.resolve(Args.cert);
        KeyFile = path.resolve(Args.key);
    }
    else {
        Usage("say where to write: --out <dir>, or --cert <file> and --key <file>");
    }

    if (!Args.force) {
        for (const File of [CertFile, KeyFile]) {
            if (fs.existsSync(File)) {
                Usage(`${File} exists. A new certificate changes the fingerprint and breaks every invite already sent; add --force if that is what you want`);
            }
        }
    }

    const { certPem, keyPem } = MakeCertificate(Names);
    fs.mkdirSync(path.dirname(CertFile), { recursive: true });
    fs.mkdirSync(path.dirname(KeyFile), { recursive: true });
    fs.writeFileSync(KeyFile, keyPem, { mode: 0o600 });
    fs.writeFileSync(CertFile, certPem, { mode: 0o644 });

    const Info = Fingerprint(certPem);
    if (Args.json) {
        process.stdout.write(JSON.stringify({ cert: CertFile, key: KeyFile, ...Info }) + "\n");
        return;
    }
    process.stdout.write([
        `Certificate: ${CertFile}`,
        `Key:         ${KeyFile}  (keep private)`,
        `Names:       ${Info.names}`,
        `Valid:       ${Info.notBefore} to ${Info.notAfter}`,
        `Fingerprint: ${Info.fingerprint}`,
        "",
        "Invites for this server carry: fp=" + Info.fingerprint,
        "",
    ].join("\n"));
}

try {
    Main();
}
catch (error) {
    process.stderr.write(`make-cert: ${error && error.message ? error.message : String(error)}\n`);
    process.exit(1);
}
