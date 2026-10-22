import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { CheckAllowlistIp, ParseIp, PeerAddress, RateLimitKey } from "../src/ip";

describe("ParseIp", () => {
    it("accepts dotted IPv4 without leading zeros", () => {
        assert.deepEqual(ParseIp("203.0.113.7")?.canonical, "203.0.113.7");
        assert.equal(ParseIp("0.0.0.0")?.class, "unspecified");
        assert.equal(ParseIp("255.255.255.255")?.class, "reserved");
        for(const Bad of ["010.0.0.1", "1.2.3", "1.2.3.4.5", "256.1.1.1", "1.2.3.-1", " 1.2.3.4", "1.2.3.4 ", "1.2.3.4\n", "0x7f.0.0.1", "1.2.3.4/32", "", "1..2.3"]){
            assert.equal(ParseIp(Bad), undefined, Bad);
        }
    });

    it("canonicalises IPv6 (RFC 5952) and unwraps IPv4-mapped addresses", () => {
        assert.equal(ParseIp("2001:0DB8:0000:0000:0000:0000:0000:0001")?.canonical, "2001:db8::1");
        assert.equal(ParseIp("2001:db8:0:0:1:0:0:1")?.canonical, "2001:db8::1:0:0:1");
        assert.equal(ParseIp("2001:db8::")?.canonical, "2001:db8::");
        assert.equal(ParseIp("::")?.canonical, "::");
        assert.equal(ParseIp("::1")?.canonical, "::1");
        assert.equal(ParseIp("2001:db8:1:2:3:4:5:6")?.canonical, "2001:db8:1:2:3:4:5:6");
        const Mapped = ParseIp("::ffff:203.0.113.9");
        assert.equal(Mapped?.version, 4);
        assert.equal(Mapped?.canonical, "203.0.113.9");
        assert.equal(ParseIp("::ffff:cb00:7109")?.canonical, "203.0.113.9");
        for(const Bad of ["fe80::1%eth0", "fe80::1%1", "2001:db8::/64", "2001:db8:::1", ":1::", "1:2:3:4:5:6:7:8:9", "1:2:3:4:5:6:7:8::", "g::1", "2001:db8::1 ", "[2001:db8::1]", "::ffff:010.0.0.1"]){
            assert.equal(ParseIp(Bad), undefined, Bad);
        }
    });

    it("classifies the special ranges", () => {
        const Cases: [string, string][] = [
            ["8.8.8.8", "public"], ["203.0.113.7", "public"], ["100.63.255.255", "public"], ["100.128.0.0", "public"],
            ["10.1.2.3", "private"], ["172.16.0.1", "private"], ["172.31.255.255", "private"], ["172.32.0.1", "public"],
            ["192.168.1.1", "private"], ["100.64.0.1", "private"], ["100.127.255.255", "private"], ["198.18.0.1", "private"],
            ["127.0.0.1", "loopback"], ["127.255.0.9", "loopback"], ["169.254.1.1", "link-local"],
            ["0.1.2.3", "unspecified"], ["224.0.0.1", "multicast"], ["239.255.255.250", "multicast"],
            ["240.0.0.1", "reserved"], ["192.0.0.8", "reserved"],
            ["2001:db8::1", "public"], ["2a00:1450:4001::1", "public"],
            ["::1", "loopback"], ["fe80::1", "link-local"], ["fd7a:115c:a1e0::1", "private"], ["fc00::1", "private"], ["fec0::1", "private"],
            ["ff02::1", "multicast"], ["::", "unspecified"], ["::1.2.3.4", "reserved"], ["64:ff9b::808:808", "reserved"], ["100::1", "reserved"],
        ];
        for(const [Text, Class] of Cases){
            assert.equal(ParseIp(Text)?.class, Class, Text);
        }
    });
});

describe("PeerAddress and RateLimitKey", () => {
    it("reports socket addresses canonically", () => {
        assert.equal(PeerAddress("::ffff:127.0.0.1"), "127.0.0.1");
        assert.equal(PeerAddress("127.0.0.2"), "127.0.0.2");
        assert.equal(PeerAddress("2001:DB8::5"), "2001:db8::5");
        assert.equal(PeerAddress(undefined), "unknown");
    });

    it("counts IPv4 per address and IPv6 per /64", () => {
        assert.equal(RateLimitKey("203.0.113.7"), "203.0.113.7");
        assert.equal(RateLimitKey("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::/64");
        assert.equal(RateLimitKey("2001:db8:1:2:bbbb::9"), "2001:db8:1:2::/64");
        assert.notEqual(RateLimitKey("2001:db8:1:3::1"), RateLimitKey("2001:db8:1:2::1"));
    });
});

describe("CheckAllowlistIp", () => {
    it("accepts single public addresses and canonicalises them", () => {
        for(const [Given, Canonical] of [["203.0.113.7", "203.0.113.7"], ["8.8.4.4", "8.8.4.4"], ["2001:DB8::0:1", "2001:db8::1"], ["::ffff:198.51.100.2", "198.51.100.2"]]){
            const Check = CheckAllowlistIp(Given, false);
            assert.equal(Check.ok, true, Given);
            assert.equal(Check.ok && Check.ip.canonical, Canonical);
        }
    });

    it("rejects ranges, garbage, unspecified, broadcast and multicast even with private allowed", () => {
        const Cases: [unknown, string][] = [
            ["0.0.0.0", "unspecified"], ["::", "unspecified"], ["0.0.0.0/0", "range_not_allowed"], ["203.0.113.0/24", "range_not_allowed"],
            ["203.0.113.1-203.0.113.9", "range_not_allowed"], ["203.0.113.1,203.0.113.2", "range_not_allowed"], ["*", "range_not_allowed"],
            ["Any", "range_not_allowed"], ["LocalSubnet", "range_not_allowed"], ["Internet", "range_not_allowed"], ["localhost", "range_not_allowed"],
            ["2001:db8::/32", "range_not_allowed"], ["255.255.255.255", "reserved"], ["224.0.0.251", "multicast"], ["ff02::fb", "multicast"],
            ["hello", "not_an_ip"], ["1.2.3.4;calc", "not_an_ip"], ["1.2.3.4; Remove-NetFirewallRule", "range_not_allowed"], ["1.2.3.4'", "not_an_ip"], ["", "not_an_ip"],
            ["1.2.3.4$(whoami)", "not_an_ip"], ["1.2.3.4`n", "not_an_ip"],
            [" 1.2.3.4", "not_an_ip"], ["fe80::1%12", "not_an_ip"], ["1.2.3.04", "not_an_ip"],
            [16909060, "not_a_string"], [null, "not_a_string"], [["1.2.3.4"], "not_a_string"], [undefined, "not_a_string"],
        ];
        for(const [Given, Reason] of Cases){
            for(const AllowPrivate of [false, true]){
                const Check = CheckAllowlistIp(Given, AllowPrivate);
                assert.equal(Check.ok, false, `${String(Given)} (allowPrivate ${AllowPrivate})`);
                assert.equal(!Check.ok && Check.reason, Reason, String(Given));
            }
        }
    });

    it("rejects private, loopback and link-local addresses unless allowed", () => {
        for(const Given of ["10.0.0.5", "192.168.1.20", "172.20.1.1", "100.101.102.103", "127.0.0.1", "169.254.3.3", "::1", "fe80::2", "fd00::5"]){
            const Refused = CheckAllowlistIp(Given, false);
            assert.equal(Refused.ok, false, Given);
            assert.equal(!Refused.ok && Refused.reason, "private", Given);
            assert.equal(CheckAllowlistIp(Given, true).ok, true, Given);
        }
    });
});
