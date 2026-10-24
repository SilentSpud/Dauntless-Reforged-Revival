<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: admin API (RX3)

The checklist for the admin API, `rust/services/admin` ([admin-api.md](../admin-api.md)). It is a new trust boundary:
a caller holding its key can kick, time out and ban players. `SECURITY_REVIEW.md` indexes it.

How to use it: before the admin API ships in a server release, copy this list into the release's review, check each
item in the code (not from memory), link the code or test that shows it, and sign at the end with the date and the
commit reviewed. An item that doesn't hold is a bug to fix before the release, not a box to leave open.

## What it is and where the boundary is

A GraphQL service for the host's tools, such as a Discord bot. It runs no moderation of its own. It calls the Node
backend's admin routes and the Director, so what it can do is bounded by what those routes do.

- **In, from callers:** HTTP on `127.0.0.1:61003` only. `POST /graphql` with `Authorization: Bearer <key>`, an
  optional `x-reforged-operator-id` (digits), and a JSON body. `GET /health` answers `{"ok":true}` without a key, for
  the host kit's ready probe. It reveals nothing.
- **In, from the host:** `admin.env` (written by `reforged-host setup` from `host.toml` and `secrets.env`) and the
  timeouts file it names.
- **In, from this machine's services:** the backend's answers (`/admin/v1/players`, `/admin/v1/online-players`,
  `/admin/v1/players/<id>/access`, `/reforged/api/Kick`, `/reforged/api/ServerStatus`) and the Director's
  `/api/instances`. They are local and trusted to be what they say, but each one is parsed defensively.
- **Out:** signed `/admin/v1` calls (the manager-operator credential,
  `paradox/backend/src/security/managerOperator.ts`), the admin key to `/reforged/api/Kick`, and GraphQL answers to
  the caller.

Code: `rust/services/admin/src/` (`input.rs` holds every check of outside input, `app.rs` the HTTP side, `schema.rs`
the fields and what they call, `upstream.rs` the calls and the signature, `hmac.rs`, `timeouts.rs`, `config.rs`,
`server.rs`). Tests: `tests/api.rs` runs the API end to end against a fake backend and Director that check the
signature and the admin key as the Node code does.

Fuzz target in `rust/fuzz`:

- `admin_request`: the caller's headers and body, the query text (through async-graphql's parser), a username, a
  reason, a timeout's `minutes`/`until`, and a Director answer. Checks that a request passes the header check only
  with exactly one `Bearer <key>` and no proxy header. Checks that an accepted name, reason or timeout end is within
  its rule, and that an instance's fields stay within their caps.

## Checklist

### Who can call it

- [ ] Every `BIND_HOST` entry is `localhost`, a `127.x` address or `::1`. Anything else stops the start, and there is
      no override (`config.rs`, `is_loopback_host`; `listens_and_calls_on_loopback_only`).
- [ ] A peer that is not loopback is refused even so (`app.rs`, `graphql`).
- [ ] The gateway never routes to port 61003: it forwards only to `GATEWAY_BACKEND_URL` and `GATEWAY_CONTENT_URL`.
      In Docker the `admin` service publishes no port (`compose.yaml`).
- [ ] A request carrying `Forwarded`, `Via`, `X-Real-IP`, `X-Reforged-Gateway` or any `X-Forwarded-*` header is
      403, before the key is looked at (`input.rs`, `check_headers`).
- [ ] The key is compared as SHA-256 digests with a constant-time fold, so neither its length nor where it differs
      shows (`hmac.rs`, `same_secret`). A repeated `Authorization` header is refused.
- [ ] The key, the backend's admin key and the operator secret are at least 32 characters, or the start fails
      (`config.rs`). setup makes each as 32 random bytes in hex (`commands/setup/secrets.rs`).
- [ ] The body must be `application/json`, at most 16 KiB, and one request, not a batch. A browser page on the host
      therefore can't send a "simple" cross-site request, and the bearer key is needed in any case.
- [ ] GET runs no query: `/graphql` answers 405 to anything but POST.

### What a query can do

- [ ] Depth is capped at 6 and complexity at 200 (`schema.rs`, `build`). Each field makes at most a few backend
      calls; a name search reads at most 100 pages of 100.
- [ ] Usernames are checked with the launcher's rule (`protocol::account::is_valid_username`) before any call.
      Account ids sent on in a path or `?ids=` match `[A-Za-z0-9_-]{1,64}` (`is_account_id`). The search term is
      percent-encoded.
- [ ] Reasons are 3 to 400 characters, control characters turned into spaces (`input.rs`, `reason`). The backend
      caps them at 500 and the timeout prefix fits inside that.
- [ ] A timeout ends in the future and within a year. `minutes` overflow is refused (`timeout_end`).
- [ ] Accounts with the backend's `admin` role can't be kicked, timed out or banned (`schema.rs`, `target`).
- [ ] A timeout is refused for an account that is banned for good or disabled, so lifting it never undoes someone
      else's ban. When it runs out, it is lifted only if the account still carries its exact reason (`sweep`;
      `a_timeout_never_lifts_a_ban_made_since_or_one_it_did_not_make`).
- [ ] When the timeouts file can't be written, the ban just made is undone rather than left permanent
      (`schema.rs`, `timeout`). A damaged file stops the start instead of forgetting every timeout (`timeouts.rs`).
- [ ] The file is written whole to `<file>.tmp` and renamed. It lives in `.workspace/config`, next to `secrets.env`.

### What it sends on

- [ ] The backend and Director URLs must be `http://` on a loopback address with no path (`config.rs`, `local_url`).
      The admin key and signatures never leave the machine.
- [ ] The `/admin/v1` signature is the manager-operator contract: HMAC-SHA256 over method, request target,
      timestamp, operator id and the body's SHA-256, joined by LF. It is checked against a value recorded from
      `managerOperator.ts` (`upstream.rs`, `signs_as_the_backend_verifies`). Timestamps never repeat, so two
      identical calls in one millisecond aren't refused as a replay (`next_timestamp`).
- [ ] The operator id is 1 to 32 digits, the caller's or `0`. The backend records it as `manager:<id>` in every
      audit record. It is the caller's word, not proof: anyone with the key can name any id.
- [ ] Every outgoing call has a 5 s limit and reads at most 1 MiB of answer.
- [ ] Backend answers are parsed into typed structs, and a shape that doesn't fit is `BACKEND_UNAVAILABLE`. The
      Director's list is checked field by field as `ParseDirectorInstances` does (`input.rs`, `parse_instances`).
- [ ] Error messages to the caller name the problem without the backend's addresses or keys. The details go to the
      log (`schema.rs`, `upstream`).
- [ ] Nothing logs a key, the operator secret or a signature. Moderation logs the account, the operator id and the
      reason.

### What changed on the backend's side

- [ ] `MYSTICPARADOX_MANAGER_ADMIN=on` in `backend.env` makes `/admin/v1/*` accept the signed credential: from
      loopback only, with no forwarding header, a timestamp within 60 s, and a signature not seen in 120 s. The
      gateway already refuses `/admin` (`rust/services/gateway/src/policy.rs`).
- [ ] `MYSTICPARADOX_MANAGER_OPERATOR_SECRET` is in `secrets.env` (owner-only), `backend.env` and `admin.env`. The
      env files are owner-only too (`write_secret_file`).
- [ ] `POST /reforged/api/Kick` sits behind `HasAdminKey` (the admin key, a loopback peer, no gateway or
      `X-Forwarded-For` header). The gateway refuses it like every `/reforged/api` route not on its public list
      (`PUBLIC_REFORGED_API`).

## paradox/ change

RD2 makes fixes the baseline for `paradox/`, not a rule. The kick needs a way to drop a live session, and nothing in
the Node backend did that: `SessionRegistry` holds the chat connections, but no route closed them on demand. The Rust
backend can't start before its gate (RD17). So `paradox/` gained one small route (2026-10-24, decision RD25):

- `paradox/backend/src/routes/reforgedApi.ts`: `POST /reforged/api/Kick {Username}`, behind `HasAdminKey`, answers
  `{UserId, Username, Closed}` (404 `not_found`, 400 `bad_request`).
- `paradox/backend/src/reforgedApiRules.ts`: `KickAccount` revokes the account's refresh sessions and unused exchange
  codes, then closes each chat connection with WebSocket code 1008 `kicked by the host`. Tested in
  `reforgedApiRules.test.ts`.
- The game's access token isn't revocable in the Node backend and stays valid until it expires. The kick doesn't
  ban, so the player can sign straight back in with their key. To keep someone out, use a timeout or a ban.
- RB7 ports this route with the rest of `/reforged/api`, and RB1's inventory has to list it.

## Changes to section 6

Section 6 keeps the host kit's `.env` files as the Node kit wrote them. This row adds to them and changes nothing
already there: two keys at the end of `secrets.env`, two lines in `backend.env` (after `REFORGED_ADMIN_KEY`), and a
new `admin.env`. `tests/node_fixtures.rs` still checks every Node-kit line byte for byte and leaves out only these.

## Fuzzing

A short run of the target is in CI. The 24-hour run before a release that ships it, from `rust/fuzz` (on Windows
the AddressSanitizer runtime must be on PATH, see `rust/fuzz/Cargo.toml`):

```bash
cargo +nightly fuzz run admin_request -- -max_total_time=86400
```

Signed: ________ (date, commit)
