# WTF Helper protocol, version 1

This is the contract between the WTF Helper desktop app and a service that relays checks to it. Types for services to copy: [`wtf-helper-protocol.ts`](wtf-helper-protocol.ts). The exact query behind every check: [`../CHECKS.md`](../CHECKS.md).

Any service can implement this protocol and ship its own build of the helper under its own name and signature. The official **WTF Helper** build talks only to `https://api.collaboraitr.com`, and the tester-only **WTF Helper Test** build talks only to `https://apidev.collaboraitr.com`. Neither can be pointed anywhere else at run time.

## Shape of the connection

The helper never listens on a port. It makes three kinds of outbound HTTPS request to one fixed host:

1. **Pair** once, with a six-digit code the person gets from the service.
2. **Hold a stream** of server-sent events. Check requests arrive on it.
3. **Post a result** for each check request.

All routes live under one base path. For the official build that is `https://api.collaboraitr.com/api/wtf/helper`, written `{base}` below.

## Envelope

Every JSON response is wrapped:

```json
{ "success": true, "data": { } }
{ "success": false, "error": "Plain message", "code": "machine_code", "recoverable": true }
```

`recoverable` appears only when trying again later can work. Error codes are listed in `WtfHelperErrorCode`.

## Device token

Pairing returns a device token. The helper keeps it in the macOS Keychain or the Windows Credential Manager and sends it on every stream and result request:

```
X-WTF-Helper-Token: <deviceToken>
```

(`Authorization: Bearer <deviceToken>` is also accepted by the collaborAItr service.) The service stores only a SHA-256 hash of the token.

## 1. Pairing

The person asks the service for a code (`POST {base}/pair/start`, on their own signed-in session). The code is six digits and lasts ten minutes. They type it into the helper, which claims it:

```
POST {base}/pair/claim
Content-Type: application/json

{ "code": "123456", "deviceName": "Pat's MacBook Air", "os": "macOS", "osVersion": "14.6.1", "helperVersion": "0.1.0" }
```

| Field | Limit |
|---|---|
| `code` | exactly 6 digits |
| `deviceName` | required, up to 80 characters |
| `os` | required, up to 40 characters |
| `osVersion` | optional, up to 80 characters |
| `helperVersion` | required, up to 40 characters |

Response `data`: `{ "deviceId": "…", "deviceToken": "…", "userId": "…" }`.

Errors: `invalid_code`, `code_used`, `code_expired`, `invalid_device`, `rate_limited`, `not_enabled`.

## 2. Stream

```
GET {base}/stream
X-WTF-Helper-Token: <deviceToken>
Accept: text/event-stream
```

Each event is one `data:` line holding JSON, followed by a blank line.

**Heartbeat**, every 10 seconds:

```json
{ "type": "heartbeat", "at": "2026-09-28T07:14:22.546Z" }
```

**Check request:**

```json
{
  "type": "check",
  "requestId": "3f1c…",
  "receipt": "eyJ2Ijox….sig",
  "approval": "person",
  "checks": [{ "check": "disk_space" }, { "check": "installed_apps", "options": { "limit": 50 } }],
  "expiresAt": "2026-09-28T07:14:52.546Z"
}
```

- `approval` is `person` (the person approved this check on the card) or `allow_all` (they chose **Allow all checks for this fix**).
- `checks` holds one to three checks, each name at most once. `options` are whole numbers inside `WTF_HELPER_OPTION_BOUNDS`.
- `receipt` is opaque and signed by the service. The helper echoes it unchanged.
- The service waits about 30 seconds for the answer. After `expiresAt` it rejects the result with `request_expired`.
- A person's computer clock is often wrong. The helper judges `expiresAt` by the service's clock, taken from the stream response's HTTP `Date` header and then from each heartbeat's `at`. It spends at most 20 seconds on a request and keeps 2 seconds back to post the result.

The stream stays open for hours. The helper treats 30 seconds without any bytes as a dropped connection and reconnects, backing off from 1 second to 30 seconds. It also reconnects after the service ends the stream, which happens on deploys.

When the device is revoked, the service ends the stream. The next connection gets `401` with `revoked`, and the helper forgets its token.

Errors on connect: `token_required`, `invalid_token`, `revoked` (all `401`), and `not_enabled` (`404`).

## 3. Result

```
POST {base}/checks/{requestId}/result
X-WTF-Helper-Token: <deviceToken>
Content-Type: application/json

{
  "receipt": "eyJ2Ijox….sig",
  "approval": "person",
  "results": [
    { "check": "disk_space", "ok": true, "summary": "Main drive: 38 GB free of 250 GB (15%).", "data": { } },
    { "check": "installed_apps", "ok": false, "error": "This check took too long." }
  ]
}
```

- Exactly one result per requested check, using the same names.
- `summary` is up to 500 characters, `error` up to 200. The whole body must stay under 256,000 bytes; the helper keeps `data` under 8,000 characters.
- `approval` and `receipt` must match the check event.

Errors: `invalid_receipt`, `request_expired` (recoverable), `wrong_device`, `approval_required`, `invalid_result`, `result_too_large`.

Nothing in a result is stored by the service; it is handed to the waiting web app and dropped. The helper does not store check output either.

## What the helper refuses

The service is the first gate. The helper checks again and refuses:

| Situation | What the helper does |
|---|---|
| Paused by the person | Runs nothing. Posts `ok: false` with "WTF Helper is paused on this computer." for every requested check. Allow all is refused too. |
| An unknown check name | Runs nothing in that request. Posts `ok: false` for every requested check. |
| Options outside the bounds, or more than three checks | Runs nothing. Posts `ok: false` for every requested check. |
| `approval` missing or not `person` / `allow_all` | Runs nothing and posts nothing. The service times out with `try_again`. |
| A missing `receipt`, a malformed `requestId`, a repeated check name, or a request already past `expiresAt` | Runs nothing and posts nothing. |

Every request, run or refused, appears in the helper window's activity log with the time, the check names, and who approved it ("You allowed" or "Allow all"). The log is kept in memory only and never holds check output.

## Revoking

- **From the service:** `DELETE {base}/{deviceId}` on the person's signed-in session. The token stops working at once.
- **From the helper:** **Disconnect** deletes the token from the keychain and stops the stream. Protocol version 1 has no route for a device to revoke its own token, so the device also stays listed in the person's account until they remove it there. It can no longer connect, because the token is gone.

## Versioning

This is version 1. A change that an existing helper or service cannot ignore gets a new version number and a new base path.
