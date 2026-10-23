<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: launcher invites, pinned TLS and the host API (RX3)

The checklist for the launcher's network client: invite parsing, the TLS connection pinned to the invite's
certificate, the HTTP client and the host API calls (MIGRATION_PLAN.md RA4, RA5, RD10). `SECURITY_REVIEW.md` indexes
it. The runtime download (`request_to_file`, RA7), the relay (RA6) and the keystore (RA8) have pages of their own.

How to use it: before the launcher's cutover, copy this list into the cutover PR, check each item in the code (not
from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An
item that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

A player's account key is the whole account: whoever has it plays as them. The launcher holds one key per server,
stored with the certificate fingerprint (`fp`) of the invite it registered through (plan section 6). Everything here
exists so that the key goes to that server and nowhere else, and so that what a server answers cannot do more than
fill in the page.

- **In, from the player:** invite links (`reforged-revival://join?...`), pasted or opened through the scheme handler,
  and pasted account keys or backup files. Untrusted: a link can come from anyone.
- **In, from a server:** every answer of ServerStatus, Register, GetUserInfo, the runtime manifest, news and the art
  pack. Untrusted: a public server is whoever the invite named, and a private-mode host is a friend's PC.
- **Out:** HTTP/1.1 requests to the invite's host and port only. Public mode (invite v2) over TLS pinned to `fp`;
  private mode (invite v1) over plain HTTP inside the tailnet (Tailscale encrypts it) or to this PC.

Code, in `rust/launcher/src-tauri/src`:

- `core/invite.rs`, `core/status.rs`, `core/username.rs`: re-exports of `protocol::invite`, `protocol::status` and
  `protocol::account` (`rust/crates/protocol`, RS1). The launcher has no parser of its own for these; it answers
  exactly as the servers' crate does, and both run `crates/protocol/tests/cases/*.json`.
- `net/pinned.rs`: the pinned TLS connection. `net/connector.rs`: the same, pooled, for the relay.
- `net/http.rs`: `request`, the only way the launcher sends HTTP to a server.
- `net/hostapi.rs`: the calls; `net/hostapi/parse.rs`: reading their answers, without the network.

Fuzz targets in `rust/fuzz`: `protocol_input` (invites, statuses, usernames and keys, through `protocol`),
`launcher_input` (the same, through the launcher's `core` paths) and `launcher_net` (`parse.rs`: Register and
GetUserInfo answers, runtime manifests, news, the art pack's index, paths and image sniffing).

## Checklist

### Invites

- [ ] Only `reforged-revival://join` with exactly the v1 or v2 parameters is read: unknown, repeated or missing
      parameters, a fragment, bad percent-encoding or more than 2048 UTF-16 units refuse the whole invite
      (`protocol::invite::parse_invite`). A `dauntless-revived://` invite is refused as `revived` and its fields are
      never read (RD5).
- [ ] The host is IPv4 in dotted-decimal or a lower-case DNS name; anything a resolver could read as a number
      (`1.2.3`, `0x7f.1`, `host.123`) is refused, so the host the page shows is the host connected to.
- [ ] A v1 (plain HTTP) invite names only a Tailscale address (100.64.0.0/10), a `*.ts.net` name or loopback, so a
      private-mode invite can never point the key at the internet in clear text.
- [ ] A v2 invite carries `fp`, 64 lower-case hex; nothing public is ever used without it.
- [ ] The server name is shown as text only: control and bidi characters refuse it, white space is collapsed, at
      most 64 characters. The share link is `https://login.tailscale.com/...` only, without credentials or a port.

### Pinned TLS

- [ ] The handshake succeeds only when the SHA-256 of the server's leaf certificate (DER) equals the invite's `fp`
      (`Pin::verify_server_cert`). There is no CA, no hostname check and no fallback: a rotated or different
      certificate fails as `Pin`, with the fingerprint it showed for the page's "certificate changed" message.
- [ ] The handshake's signature is still verified with the pinned certificate's key (`verify_tls12_signature`,
      `verify_tls13_signature`), so a copy of the certificate without its key does not pass.
- [ ] Nothing is written before the handshake succeeds: not a request line, not a header
      (`a_wrong_or_rotated_certificate_is_refused_before_anything_is_sent`, and in `net/hostapi/tests.rs` the fake
      server sees no request from a mismatching client).
- [ ] An IP address is never sent as SNI. Only rustls' safe default protocol versions; ALPN `http/1.1` only.
- [ ] Every new connection is pinned again, including each pooled connection the relay opens (`connector.rs`).

### The HTTP client

- [ ] `check_endpoint` runs before every request: a valid host, a port from 1, a valid fingerprint when there is one,
      and plain HTTP only to a private-mode host. Any other address is refused before a socket is opened.
- [ ] The request goes to the endpoint's host and port whatever the path says; the path must start with `/`.
      Redirects are never followed (a 302 is returned as a 302).
- [ ] The whole request (connect, answer and body) has one deadline; a cancel token aborts it; bodies over the
      call's `max_bytes` fail with `TooLarge` while reading, not after.
- [ ] Plain HTTP to a TLS port is a network error, not a pin error, so a server that dropped TLS is not mistaken for
      a changed certificate.
- [ ] `RequestOptions`' `Debug` prints header names only and the body's length, never a header value
      (`options_never_print_header_values`). `HttpError` carries no request data.

### The account key

- [ ] The key is sent only in `x-reforged-user-api-key`, built only by `key_headers`: a plausible key (8-128
      characters of `[A-Za-z0-9_-]`) and an endpoint that is pinned or private-mode. Otherwise no header is sent at
      all, for ServerStatus and GetUserInfo alike (Reforged's rule; DR sent it to GetUserInfo unconditionally).
- [ ] Register, the runtime manifest, news, the art pack and `/dauntless-status` never carry the key.
- [ ] The key a server returns from Register is kept only if it is plausible, so it can go on the game's command line
      without quoting (`read_register`).
- [ ] The key is bound to the certificate: the endpoint a key is sent to is built from the stored server, whose `pin`
      is the `fp` stored with the key (controller and keystore, RA8 and RA13). Check there that no call builds an
      endpoint with a fingerprint other than the stored one, and that a changed certificate goes through the page's
      confirmation first.
- [ ] No module here logs. Check, across the launcher, that no `tracing` call formats a key, an `Endpoint` with
      options, a request's headers or a Register answer, and that the key never crosses the bridge (plan section 6).

### Answers from a server

- [ ] Every answer is read as untrusted JSON: a wrong type, a missing field or an unexpected value is dropped or
      refused, never trusted, and nothing panics (`launcher_net`, `protocol_input`).
- [ ] ServerStatus: text cleaned of control and bidi characters and capped, numbers clamped, URLs `https` only,
      at most 500 players and 100 instances; `limited` answers list nobody.
- [ ] GetUserInfo: the user id matches `^[A-Za-z0-9_-]{1,128}$` and the username is cleaned text of at most 32
      characters, since both go on the game's command line; anything else is `server_error`. A banned or disabled
      account shows the server's reason, cleaned and capped, or nothing.
- [ ] The runtime manifest names exactly `MysticParadox.dll` and `winmm.dll`, each once, with a lower-case SHA-256
      and a size from 1 byte to 64 MiB; anything else refuses the whole manifest, so a server can never name a file
      the launcher would write.
- [ ] News, branding: at most 30 items and 8 backgrounds; art paths match the branding pattern without `..`; an
      image's type comes from its first bytes, not the server's `Content-Type`; images at most 15 MiB.

### Fuzzing

- [ ] `protocol_input`, `launcher_input` and `launcher_net` run briefly on every PR with the other targets (RD10).
- [ ] A 24-hour run of each with no crash, before the cutover:

```bash
cargo +nightly fuzz run launcher_net -- -max_total_time=86400
cargo +nightly fuzz run launcher_input -- -max_total_time=86400
```

## Changes from Dauntless Revived

- The key header is `x-reforged-user-api-key` and the routes are under `/reforged/api` (DR: `x-undaunted-...`,
  `/undaunted/api`).
- GetUserInfo follows the same key rule as ServerStatus; DR sent the key unconditionally.
- DR's `DeviceSignals`, `HuntRegion` and `PlayingEnded` calls and its 1.4.4 content manifest are not ported; the
  runtime manifest replaces the content manifest.
- The launcher's own copies of the invite, status and username rules are gone; it uses `protocol`'s.

Signed: ________ (date, commit)
