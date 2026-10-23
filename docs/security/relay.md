<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: the launcher's local relay (RX3)

The checklist for the local relay in the Tauri launcher, `rust/launcher/src-tauri/src/net/relay.rs` and
`net/relay_owner.rs` (MIGRATION_PLAN.md RA6, RD10, RD21). `SECURITY_REVIEW.md` indexes it.

How to use it: before the launcher's cutover, copy this list into the cutover PR, check each item in the code (not
from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An
item that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

The game runtime sends the 1.14.7 client's backend calls to `REFORGED_BACKEND_URL=http://127.0.0.1:61100`. The relay
listens there while the game runs and forwards every request, and the chat WebSocket, to the player's server. The
requests carry the player's account key and tokens, so whoever gets the relay to forward for them acts as the player,
and whoever gets the relay to send to the wrong server gets the key.

- **In:** plain HTTP/1.1 and WebSocket upgrades on `127.0.0.1:61100` (`REFORGED_RELAY_PORT` changes it), from
  any process on the PC unless the owner check refuses it. Web pages in a browser on the same PC can reach it too
  (DNS rebinding, `fetch`, `new WebSocket`).
- **In, QoS only:** a second, best-effort listener on `127.0.0.1:80` that answers the QoS page and nothing else.
- **Out, public mode:** TLS to the gateway, every connection pinned to the invite's certificate fingerprint
  (`net/pinned.rs`, `net/connector.rs`).
- **Out, private mode:** plain HTTP to a Tailscale address or MagicDNS name, or to this PC.
- **Owner check (Windows):** the TCP table from `%SystemRoot%\System32\netstat.exe -ano -p tcp`.

Code: `net/relay.rs` (listeners, forwarding, upgrades), `net/relay/local.rs` (the local-request rules, the QoS path,
the port override), `net/relay_owner.rs` and `net/relay_owner/table.rs` (the owner check). Shared rules from
`crates/proxy` (hop-by-hop headers, the tunnel).

Fuzz target: `relay_input` in `rust/fuzz` (the peer address, Host, Origin and `Sec-Fetch-*` checks, the QoS path,
the port override, and the TCP table rows the owner check reads). It checks that nothing the local rules accept
comes from off the PC, names another host or carries a web page's marks, and that the owner check only ever reads
one exact client row.

## Checklist

### Who can use it

- [ ] Both listeners bind `127.0.0.1` only (`bind`), and a peer that is not loopback (IPv4-mapped included) is
      refused with `not_local` anyway (`local::request_problem`).
- [ ] Every request and upgrade needs a loopback `Host` (`localhost`, `::1`, `127.x.y.z`, any port); anything else,
      a missing Host or one that is not text is `403 forbidden` before anything is forwarded (DNS rebinding).
- [ ] A plain request with any `Sec-Fetch-*` header is refused (`browser`); an `Origin` that is not a loopback host,
      or not text, is refused on requests and upgrades (`origin`). Tests:
      `requests_and_websockets_from_web_pages_are_refused`, `local_request_rules`.
- [ ] Windows: with `relay_owner::game_owner_check` as `RelayOptions::authorize`, a connection is served only when
      the TCP table's one client row for it names the process id of the game the launcher started. No game, no
      table, an unreadable or duplicate row, a wrong pid, a non-loopback address or more than 64 waiting checks all
      refuse (`403 local_process_denied` and `Connection: close`; a bare 403 for an upgrade). The check runs once
      per connection, before the first request is read, and the QoS answer on the relay port is behind it too.
      Tests: `on_windows_only_the_registered_game_process_is_served` (a real `ping.exe` registered as the game, the
      test's own connection refused), `checks_share_fresh_snapshots_and_fail_closed`,
      `the_client_row_and_the_exact_pid_never_the_server_row`.
- [ ] The table is never cached: requests waiting together share one fresh `netstat` run, a missing row gets one
      retry 25 ms later, and localized state labels are never compared.
- [ ] Not Windows: `relay_owner::loopback_only_without_owner_check` returns no check, so any local process passes
      the owner step, as Reforged's Electron relay does on every platform. This is the RD21 fallback until RA10
      decides whether the game's process can be found under Proton. Test:
      `elsewhere_the_relay_serves_loopback_only_without_an_owner_check`.

### Where it sends

- [ ] Public mode: every new upstream connection, pooled or for an upgrade, is pinned to the invite's fingerprint;
      a mismatch sends nothing (no request line, no headers, no body), answers `502 certificate_mismatch` (a bare
      502 for an upgrade) and reports the certificate actually shown. Test:
      `a_server_with_another_certificate_gets_nothing`.
- [ ] Private mode (no fingerprint) starts only for `protocol::invite::is_private_mode_host`: a Tailscale address,
      a `*.ts.net` name or loopback. Anything else fails to start (`ListenFailed`). Test:
      `plain_http_only_to_tailscale_or_this_pc`.
- [ ] Every connection goes to the target's host and port, whatever the request names: the request target is put
      under a fixed `http://relay-upstream` authority, `Host` is replaced with the target's, and an absolute-form
      target cannot redirect the connection.
- [ ] Paths are forwarded exactly as sent, `/__origin/<host><path>` included; the relay never unwraps them, so a
      runtime path cannot make the relay connect anywhere else (the gateway and backend unwrap and check them).
      Test: `origin_paths_pass_through_unchanged`.

### What crosses

- [ ] Hop-by-hop headers and every header `Connection` names are dropped both ways, plus `Host` and
      `HTTP2-Settings` on the way out (`proxy::end_to_end`). Upgrades keep only `Upgrade` and send
      `Connection: Upgrade`. Tests: `forwards_method_path_query_headers_and_body_over_the_pinned_connection`,
      `forwarded_headers_drop_hop_by_hop_and_connection_named_ones`.
- [ ] A `Transfer-Encoding` other than `chunked` is refused (501) rather than reinterpreted (no request smuggling
      between the game's framing and the gateway's).
- [ ] An upgrade the server does not accept gets the server's status and at most 64 KiB of its body, then the
      connection closes; an accepted one is joined byte for byte (`proxy::tunnel`) and cut after 24 hours of
      silence both ways or when the relay stops.
- [ ] `GET` and `HEAD` to `/qos`, `/qos/...` or `/qos?...` (any case) on the relay port are answered locally with
      the fixed pong page and never forwarded; other methods and paths that only look similar are forwarded.
      Test: `qos_is_answered_locally_and_never_forwarded`.
- [ ] The QoS listener answers only `/` and QoS paths, after the same local rules (404 otherwise), forwards
      nothing and has no owner check (it holds nothing a process could use). A busy port 80 is logged and ignored.
      Tests: `the_qos_listener_answers_qos_and_the_root_only_and_closes_with_the_relay`,
      `a_busy_qos_port_is_logged_and_ignored`.
- [ ] Nothing the relay logs contains a header, a body, a path or the key: the log lines are fixed text plus the
      refusal reason, the target host and port, and the upstream error.

### Lifetime

- [ ] `stop` cancels both listeners, every kept-alive connection and every tunnel, and frees both ports. Test:
      `a_taken_port_a_server_that_is_down_and_stop_freeing_the_port`.
- [ ] A port already taken is reported as `PortBusy`, never shared.

## Open points

These do not block RA6 but must be settled, or accepted in writing, before the cutover:

- **Port takeover on Windows.** The listeners are bound without `SO_EXCLUSIVEADDRUSE`, so a process running as the
  same user could bind the same address with `SO_REUSEADDR` and receive the game's connections (and the key). The
  owner check does not cover this: it checks who connects, not who listens. Fixing it needs the socket option, which
  means `socket2` or the `windows` crate's `Win32_Networking_WinSock` feature in the launcher's `Cargo.toml` (not
  owned by RA6). DR and the Electron relay have the same gap.
- **Who wires the check.** The controller's relay start (RA13) must pass `relay_owner::game_owner_check` with the
  pid of the game it started, as DR's `start_relay` did; until then nothing calls it.
- **Linux.** No owner check at all until RA10 (RD21).
- **Slow local clients.** No header-read timeout on the relay's own listeners (the Electron relay had Node's
  defaults). Only a local process can hold connections open, and the owner check refuses others on Windows.

## Fuzzing

A short run of `relay_input` happens on every PR (CI's `fuzz` job). The run before the cutover, from `rust/fuzz`:

```bash
cargo +nightly fuzz run relay_input -- -max_total_time=86400
```

## Changes from the Electron relay

- The owner check is DR's, kept on Windows (RD21); Reforged's Node relay had none.
- The relay itself is DR's Rust relay with Reforged's changes: port 61100, private mode, the QoS page on both
  listeners and the start log naming the mode.
- A kept-alive connection the gateway closed is retried by hyper's pool rather than the Node relay's own one retry
  of a bodiless request.

Signed: ________ (date, commit)
