# Security

WTF Helper reads information about a person's computer, so we treat every report seriously.

## Reporting a problem

Please do not open a public issue for a security problem. Use GitHub's private reporting instead: the **Security** tab of this repository, then **Report a vulnerability**. We reply within a few working days.

Helpful details: the build (WTF Helper or WTF Helper Test) and version, the operating system, and steps to reproduce.

## What the helper promises

- It only reads. No check changes files, settings or programs, and there is no shell or free-form command.
- It runs only the queries listed in [CHECKS.md](CHECKS.md), as the signed-in user, never as administrator.
- It refuses unknown checks, checks without an approval, and every check while paused.
- It never listens on a network port. It makes outbound HTTPS calls to one host that is fixed when the app is built: `api.collaboraitr.com` for WTF Helper, `apidev.collaboraitr.com` for WTF Helper Test.
- The device token is kept only in the macOS Keychain or the Windows Credential Manager.
- Check output is not written to disk by the helper.

A report that breaks any of these is in scope. So is anything in the pairing, the relay client, the approval handling, or the bundled osquery's configuration.

## Signing keys

The Apple Developer ID certificate, the notarization key, the Windows signing credentials and the updater key are never in this repository. They exist only as encrypted CI secrets, referenced by name in `.github/workflows/release.yml`.

## Audits

Tauri 2 (Radically Open Security, 2024) and osquery (NCC Group 2015/2016; Trail of Bits 2022) have been audited. Those audits cover their own code, not this app. WTF Helper will have its own independent audit before its public launch, and the report will be published next to the download.
