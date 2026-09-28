# WTF Helper

A small desktop app that lets your [collaborAItr](https://collaboraitr.com) guide check your computer, only after you say yes, without changing anything.

When something on your computer isn't working, the guide can ask to look at things like how full your drives are, which programs are using the most memory, or whether the firewall is on. You see each request, you approve it, and for lists you see exactly what will be sent before it goes. WTF Helper only reads. It never changes files, settings or programs, and no person ever connects to your computer.

> **Status: in testing.** There is no public download yet. Builds are unsigned until collaborAItr's signing accounts exist, and are given only to named testers ([TESTING.md](TESTING.md)). The only place WTF Helper will ever be offered is collaboraitr.com.

## What it can and can't do

- **Eight read-only checks,** each a fixed [osquery](https://osquery.io) query: computer overview, disk space, network, security settings, busy programs, programs that start by themselves, recent crashes, installed apps. Every query is published in [CHECKS.md](CHECKS.md).
- **You approve every check,** or choose "Allow all checks for this fix" in collaborAItr for one fix at a time.
- **Pause** refuses every check, Allow all included. **Disconnect** forgets the connection.
- **Activity log:** every check, when it ran, and who allowed it. Kept only while the app is open; results are not saved.
- **No open ports.** The helper only makes outbound HTTPS calls to one host that is fixed when it is built.
- It runs as you, never as administrator. If something needs administrator rights (BitLocker status on Windows), it says so instead of asking.

## Two builds

| | WTF Helper | WTF Helper Test |
|---|---|---|
| Connects only to | `api.collaboraitr.com` | `apidev.collaboraitr.com` |
| Bundle id | `com.collaboraitr.wtfhelper` | `com.collaboraitr.wtfhelper.test` |
| Looks like | dark icon | orange icon, TEST badge |
| Given to | everyone, once signed | testers only, never linked |

Neither build can be pointed at another server. That is on purpose: otherwise a scammer could say "install WTF Helper and type this code" and pair it with their own server. The window always shows which service it is connected to.

## How it works

1. In collaborAItr, the person asks to connect the helper and gets a 6-digit code (valid 10 minutes).
2. They type it into WTF Helper, which claims it and receives a device token. The token goes into the macOS Keychain or Windows Credential Manager.
3. The helper holds a server-sent event stream open. When the person approves a check in collaborAItr, the request arrives on the stream with a signed receipt.
4. The helper checks the request again (known check, allowed options, approval present, not paused), runs the fixed queries, removes the home folder and username from the output, and posts the result with the receipt.

The full protocol, which any service can implement: [protocol/PROTOCOL.md](protocol/PROTOCOL.md), with TypeScript types in [protocol/wtf-helper-protocol.ts](protocol/wtf-helper-protocol.ts).

## Building from source

Requirements: Rust (stable), Node.js 22, and on Windows the WebView2 runtime (already part of Windows 11).

```bash
npm ci
npm run fetch-osquery    # downloads osquery 5.23.1 and checks its SHA-256
npm run dev:test         # run WTF Helper Test
npm run build:test       # build WTF Helper Test
npm run build            # build WTF Helper
```

Tests:

```bash
cd src-tauri
cargo test                                                  # unit and relay tests
cargo test --features test-build                            # the same, for the Test build
cargo test --test live_osquery -- --ignored --nocapture     # every check against the real osquery
```

For work on the relay itself, `npm run dev:local` builds a copy pinned to `http://localhost:3001`. It is never released.

## Layout

| Path | What |
|---|---|
| `protocol/` | The published protocol and its TypeScript types |
| `CHECKS.md` | Every query, what each check sends, and which tables need administrator rights |
| `src-tauri/src/flavor.rs` | The pinned host for each build |
| `src-tauri/src/gate.rs` | The helper's own refusals: unknown checks, bad options, no approval, paused |
| `src-tauri/src/checks/` | The fixed queries, redaction, and the eight checks |
| `src-tauri/src/relay.rs` | Pairing, the stream and result calls |
| `src-tauri/src/helper.rs` | Stream reconnects, Pause, Disconnect, the activity log |
| `ui/` | The window: plain HTML, CSS and JavaScript, no framework |
| `.github/workflows/` | CI on macOS and Windows; release builds that sign only when the secrets exist |

## Release signing

`release.yml` signs and notarizes the macOS build when these secrets exist: `APPLE_CERTIFICATE` (base64 .p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_P8` (the .p8 file's contents). It signs the Windows build with Azure Artifact Signing when `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` and `AZURE_TENANT_ID` exist as secrets and `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT` and `AZURE_SIGNING_PROFILE` as repository variables. Without them, it builds unsigned installers named `…-UNSIGNED`. A `v*` tag creates a draft release, visible only to people with access to this repository.

## License and name

Apache License 2.0; see [LICENSE](LICENSE) and [NOTICE](NOTICE). The license does not cover the name "WTF Helper", the name collaborAItr, or the logo, so forks must use their own name and icon, and sign with their own identity. Security reports: [SECURITY.md](SECURITY.md).
