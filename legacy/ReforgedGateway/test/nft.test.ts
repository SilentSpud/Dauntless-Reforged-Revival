import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { describe, it } from "node:test";
import { BuildScriptFor } from "../src/allowlist/firewall";
import { BuildNftScript, CanManageNft, DefaultNft, NftRunner } from "../src/allowlist/nft";

const EMPTY = [
    "table inet reforged_revival {}",
    "delete table inet reforged_revival",
    "table inet reforged_revival {",
    "\tset players4 {",
    "\t\ttype ipv4_addr",
    "\t}",
    "\tset players6 {",
    "\t\ttype ipv6_addr",
    "\t}",
    "\tchain input {",
    "\t\ttype filter hook input priority -5; policy accept;",
    "\t\tiifname \"lo\" accept",
    "\t\tip saddr @players4 udp dport 8770-8777 accept",
    "\t\tip6 saddr @players6 udp dport 8770-8777 accept",
    "\t\tudp dport 8770-8777 drop",
    "\t}",
    "}",
    "",
].join("\n");

const TWO_ON_ETH0 = [
    "table inet reforged_revival {}",
    "delete table inet reforged_revival",
    "table inet reforged_revival {",
    "\tset players4 {",
    "\t\ttype ipv4_addr",
    "\t\telements = { 203.0.113.7, 198.51.100.2 }",
    "\t}",
    "\tset players6 {",
    "\t\ttype ipv6_addr",
    "\t\telements = { 2001:db8::1 }",
    "\t}",
    "\tchain input {",
    "\t\ttype filter hook input priority -5; policy accept;",
    "\t\tiifname \"lo\" accept",
    "\t\tiifname \"eth0\" ip saddr @players4 udp dport 8770-8777 accept",
    "\t\tiifname \"eth0\" ip6 saddr @players6 udp dport 8770-8777 accept",
    "\t\tiifname \"eth0\" udp dport 8770-8777 drop",
    "\t}",
    "}",
    "",
].join("\n");

const Options = { table: "reforged_revival", iface: undefined };

describe("the nftables script", () => {
    it("is exactly this for an empty set: the game ports drop everything (closed)", () => {
        assert.equal(BuildNftScript([], "8770-8777", Options), EMPTY);
    });

    it("is exactly this for three addresses on one interface: only they reach the game ports", () => {
        assert.equal(BuildNftScript(["203.0.113.7", "2001:db8::1", "198.51.100.2"], "8770-8777", { table: "reforged_revival", iface: "eth0" }), TWO_ON_ETH0);
    });

    it("refuses anything but canonical single addresses, a port range, a plain table and interface name", () => {
        for(const Bad of ["2001:DB8::1", "1.2.3.4/32", "0.0.0.0", "any", "1.2.3.4 } ; flush ruleset ; {", "", "010.1.1.1", "224.0.0.1"]){
            assert.throws(() => BuildNftScript([Bad], "8770-8777", Options), /refusing/, Bad);
        }
        assert.throws(() => BuildNftScript(["203.0.113.7"], "8770-8777 accept", Options), /ALLOWLIST_PORTS/);
        assert.throws(() => BuildNftScript([], "8770-8777", { table: "t; flush ruleset", iface: undefined }), /ALLOWLIST_NFT_TABLE/);
        assert.throws(() => BuildNftScript([], "8770-8777", { table: "reforged_revival", iface: "eth0\" accept" }), /ALLOWLIST_INTERFACE/);
    });

    it("is what the helper builds when its firewall is nftables, and PowerShell otherwise", () => {
        const Nft = { binary: "/usr/sbin/nft", ...Options };
        assert.equal(BuildScriptFor({ firewall: "nftables", ports: "8770-8777", nft: Nft }, []), EMPTY);
        assert.match(BuildScriptFor({ firewall: "windows", ports: "8770-8777", nft: Nft }, []), /Set-NetFirewallRule/);
    });

    it("finds nft at a fixed path", () => {
        assert.match(DefaultNft(), /^\/(usr\/)?sbin\/nft$/);
    });
});

// Opt-in (REFORGED_TEST_NFT=1, Linux, root or CAP_NET_ADMIN): really applies the scripts, to a
// table of its own on ports nothing uses, and deletes the table afterwards.
const Live = process.platform === "linux" && process.env.REFORGED_TEST_NFT === "1" && CanManageNft(DefaultNft());

describe("nftables for real (REFORGED_TEST_NFT=1)", { skip: !Live && "set REFORGED_TEST_NFT=1 on Linux as root to run" }, () => {
    const Nft = DefaultNft();
    const Table = "reforged_revival_test";
    const Ports = "64770-64771";
    const List = () => spawnSync(Nft, ["list", "table", "inet", Table], { encoding: "utf8" });

    it("creates, replaces and empties the table atomically", async () => {
        const Run = NftRunner(Nft);
        try{
            await Run(BuildNftScript(["203.0.113.7", "2001:db8::1"], Ports, { table: Table, iface: undefined }));
            let Out = List();
            assert.equal(Out.status, 0, Out.stderr);
            assert.match(Out.stdout, /203\.0\.113\.7/);
            assert.match(Out.stdout, /2001:db8::1/);
            assert.match(Out.stdout, /udp dport 64770-64771 drop/);

            // A second apply replaces the first (the table already exists).
            await Run(BuildNftScript(["198.51.100.9"], Ports, { table: Table, iface: undefined }));
            Out = List();
            assert.match(Out.stdout, /198\.51\.100\.9/);
            assert.doesNotMatch(Out.stdout, /203\.0\.113\.7/);

            // Empty: still there, closed.
            await Run(BuildNftScript([], Ports, { table: Table, iface: undefined }));
            Out = List();
            assert.doesNotMatch(Out.stdout, /elements/);
            assert.match(Out.stdout, /udp dport 64770-64771 drop/);

            await assert.rejects(Run("this is not nftables\n"), /nft exited with/);
        }
        finally{
            spawnSync(Nft, ["delete", "table", "inet", Table], { stdio: "ignore" });
        }
    });
});
