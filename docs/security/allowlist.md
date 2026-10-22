<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: allowlist helper and its IPC (RX3)

The checklist for the allowlist helper, the `allowlist` binary of `rust/services/gateway` (MIGRATION_PLAN.md RS4,
RD10, RD20). `SECURITY_REVIEW.md` indexes it.

How to use it: before the helper's cutover, copy this list into the cutover PR, check each item in the code (not
from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An
item that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

The helper keeps the game's UDP ports open only to the addresses of players who logged in. It runs with the most
rights of any server component: administrator on Windows (a scheduled task with highest privileges), root or
`CAP_NET_ADMIN` on Linux (the `reforged-allowlist` systemd unit). Anything that can make it run a command or open the
ports to everyone defeats the allowlist.

- **IPC in:** plain HTTP on loopback (`ALLOWLIST_BIND`, `127.0.0.1` or `::1`, port `ALLOWLIST_PORT`, default 61005).
  The gateway sends `POST /allow` with `{"ip": "<address>"}` and the `x-allowlist-secret` header; `GET /status` reads
  the state. Any local process can connect, so the secret is the authentication.
- **Out:** the firewall. `ALLOWLIST_FIREWALL=windows` changes one Windows Firewall rule through COM
  (`INetFwPolicy2`, no PowerShell). `ALLOWLIST_FIREWALL=nftables` swaps one nftables table with `nft -f`, at a fixed
  path, with no shell.
- **Files:** the `.env` the host kit writes (trusted: whoever can write it chooses what runs elevated), the audit log
  and the state file (written by the helper, read back at start).

Code: `rust/services/gateway/src/allowlist/` (`config.rs`, `ports.rs`, `nft.rs`, `firewall.rs`, `windows.rs`,
`helper.rs`, `state.rs`, `sync.rs`), `src/ip.rs` (`check_allowlist_ip`) and `src/bin/allowlist.rs`.

Fuzz target: `allowlist_input` in `rust/fuzz` (the `/allow` body and its `Content-Length`, the state file, and the
ports, table and interface that reach a script). It also checks that nothing the checks accept changes the shape of
the nftables script or the Windows rule.

## Checklist

### The IPC endpoint

- [ ] It binds only to `127.0.0.1` or `::1`; any other `ALLOWLIST_BIND` stops the start (`config.rs`). A peer that
      is not loopback gets 403 even so (`helper.rs`, `handle`).
- [ ] Every request but a 404 or 405 needs `x-allowlist-secret`. The comparison hashes both sides with SHA-256 first,
      so its time says nothing about the secret (`secret_matches`). The secret is at least 32 characters
      (`check_secret`) and never written to the audit log or the console (`tests/allowlist.rs` checks the audit log).
- [ ] Only `POST /allow` and `GET /status` exist; any other path is 404 and a wrong method 405.
- [ ] Bodies are at most 1024 bytes, refused by `Content-Length` before reading and by a limited read after
      (413). A body that is not JSON is 400. The connection limits hold: 64 at once, 5 s for the headers, 30 s per
      connection.
- [ ] Failed secrets, refused addresses and a full list are audited at most once a minute per kind, with a count of
      the ones left out, so a local process cannot flood the audit log.
- [ ] `/status` shows the live addresses and the last apply, nothing about the secret or the configuration files.

### What reaches the firewall

- [ ] Only an address that passes `check_allowlist_ip` is kept: one IPv4 or IPv6 address in canonical form, no
      range, CIDR, list, zone id, leading zero, white space or Windows keyword (`Any`, `LocalSubnet`...);
      unspecified, multicast and reserved addresses always refused; private, loopback and link-local only with
      `ALLOWLIST_ALLOW_PRIVATE=1`.
- [ ] `plan` checks every address again (it must already be canonical) and the port range, so no caller can put
      anything else into a script or a rule. `nft_script` checks the table and interface names again too.
- [ ] `ALLOWLIST_PORTS` is one port or `low-high` within 1-65535 and nothing else (`ports.rs`). The host kit writes
      it, widened for `--max-hunts`.
- [ ] `ALLOWLIST_NFT_TABLE` is a letter then up to 31 letters, digits or `_`; `ALLOWLIST_INTERFACE` is at most 15
      characters from `[A-Za-z0-9_.@-]`, starting with a letter or digit. Neither can close a quote or a brace in the
      script.
- [ ] At most `ALLOWLIST_MAX_ENTRIES` addresses (default 256, at most 1000); past that, `/allow` answers 503 and
      the rule does not grow. Each address expires `ALLOWLIST_TTL_SECONDS` after its last report (default 600).
- [ ] Changes are debounced: at most one firewall change per `ALLOWLIST_MIN_INTERVAL_MS` (default 3000), however
      fast addresses arrive (`sync.rs`).

### nftables (Linux)

- [ ] nft runs at `ALLOWLIST_NFT`, which must be an absolute path, else the first of `/usr/sbin/nft` and
      `/sbin/nft` that exists. PATH is never searched. The argument list is fixed (`-f -` or `-f <file>`,
      `list tables`); no shell is involved.
- [ ] Each change is one transaction that replaces the whole table (`table inet T {}`, `delete table inet T`, the new
      table). There is never a moment with the ports open to everyone or with the table missing; a script nft
      refuses changes nothing.
- [ ] The chain's first rule is `iifname "lo" accept`, so loopback is never filtered. The other three rules match
      the game ports only: from `@players4` or `@players6` accept, anything else on those ports drop. The chain's
      policy is `accept`, so the helper cannot cut the host off anything else.
- [ ] The helper only ever touches its own table (`reforged_revival` by default). A host that ran the Node helper
      has the same table, and the first change replaces it.
- [ ] The script goes to nft on stdin. Only if stdin fails the first time and a file works does the helper switch to
      files: a new folder of mode 0700 under the temporary folder, `rules.nft` of mode 0600 created with
      `create_new`, both removed after the run (`nft.rs`, `private_script`; tested on Linux with a stand-in nft).
- [ ] An nft run is killed after 30 s. Its stderr is kept to the last 4000 bytes and an error shows the first
      1000.
- [ ] Real mode refuses to start unless it runs on Linux and `nft list tables` succeeds within 15 s.

### Windows Firewall

- [ ] The helper touches only rules whose display name is `Dauntless Reforged Revival game ports (allowlist)`, the
      Node helper's. The first one is set to the plan and any other is disabled, so exactly one is in effect; a
      Node-made rule is taken over, not duplicated.
- [ ] The rule is inbound, Allow, UDP, the game ports only, remote addresses exactly the plan's, every profile
      (`windows_change`, pinned by a unit test). An empty plan disables it, so Windows drops the game ports' traffic
      by default.
- [ ] No PowerShell runs. `ALLOWLIST_POWERSHELL` from an existing `.env` is accepted and ignored.
- [ ] Real mode refuses to start unless the process is elevated (`is_elevated`, a token query, read-only), and
      `ALLOWLIST_FIREWALL=windows` refuses to start on another platform.

### Failure and restart

- [ ] At start the helper restores the saved addresses that have not expired (never for longer than one TTL from
      now, whatever the file says, and only addresses that pass the same checks), then applies exactly that set. The
      rule never keeps addresses from before a crash beyond what the state file justifies.
- [ ] On SIGINT, SIGTERM or Ctrl+Break it stops listening, saves the live set and closes the ports. A failed or slow
      shutdown ends after 90 s.
- [ ] `--close-ports` closes the ports once and exits, for stop scripts (a supervisor that kills the process never
      lets it close them itself).
- [ ] A crash leaves the last applied set, never wider: there is no state in which the rule allows any address.
- [ ] A failed firewall change is retried with backoff and audited (`rule_failed`); `/status` shows it.
- [ ] The state file is written as a temporary file of mode 0600 and renamed into place.

### Configuration

- [ ] `ALLOWLIST_DRY_RUN` must be set to 1 or 0; anything else stops the start. Dry run never touches the firewall
      and writes what it would do to the audit log (the exact nft script, or the Windows rule's settings).
- [ ] Only the `ALLOWLIST_*` names the host kit writes are read; Dauntless Revived's `DR_NFT_BINARY`,
      `DR_NFT_TABLE` and `DR_FIREWALL_INTERFACE` are not.
- [ ] The `.env` and the helper's binary are writable only by an administrator (Windows) or root (Linux): they decide
      what runs elevated. This is the host kit's job (RH rows); check it on the canary.

### Fuzzing

- [ ] `allowlist_input` runs briefly on every PR with the other targets (RD10; the CI fuzz job is RR3's).
- [ ] A 24-hour run of `allowlist_input` with no crash, before the cutover:

```bash
cargo +nightly fuzz run allowlist_input -- -max_total_time=86400
```

## Live checks (maintainer)

Neither of these runs in CI. Each uses a table or rule of its own on ports nothing uses (64770-64771) and deletes it
afterwards.

Debian 13, as root, with nftables installed (the result says whether nft read the script on stdin or needed a file):

```bash
sudo apt install nftables
cd rust
sudo -E env "PATH=$PATH" REFORGED_TEST_NFT=1 cargo test -p gateway --lib nftables_for_real -- --nocapture
sudo nft list tables
```

The last command must not list `reforged_revival_test`.

Windows, in an elevated prompt:

```powershell
cd rust
$env:REFORGED_TEST_WINDOWS_FIREWALL = "1"
cargo test -p gateway --lib windows_firewall_for_real -- --nocapture
Get-NetFirewallRule -DisplayName "Dauntless Reforged Revival allowlist test (delete me)" -ErrorAction SilentlyContinue
```

The last command must print nothing.

## Changes from the Node helper

- Windows Firewall goes through COM instead of PowerShell (DR). A rule the COM code creates has a generated id
  instead of the Node helper's `Name` (`ReforgedRevival-GamePorts-Allowlist`); rules are found by display name, which
  both keep.
- nft reads the script on stdin first, and only falls back to the Node helper's private file (RD20).
- A dry run logs the exact nft script for nftables (as Node did) and the rule's settings for Windows Firewall
  (Node logged a PowerShell script there).

Signed: ________ (date, commit)
