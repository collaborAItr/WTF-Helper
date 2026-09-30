# Testing WTF Helper before the builds are signed

Until collaborAItr's Apple Developer ID and Azure Artifact Signing accounts exist, every build from CI is **unsigned**. Unsigned builds are for named testers only. They are never linked from collaborAItr, from inside the app, or from anywhere public: the warnings they trigger look exactly like malware warnings, and most people could not, and should not, get past them.

## Which build to use

| Build | Talks to | Use it for |
|---|---|---|
| **WTF Helper Test** (orange icon, TEST badge) | `apidev.collaboraitr.com` (the appdev site) | Almost all testing |
| **WTF Helper** | `api.collaboraitr.com` (production) | Only a final check before release |

The two have different bundle ids, so both can be installed side by side.

## Getting a build

A maintainer runs the **Release builds** workflow (Actions → Release builds → Run workflow) and sends the installer from the run's artifacts to the tester directly. Artifacts are kept for 14 days. Each artifact includes a `SHA256SUMS` file; testers can compare it with `shasum -a 256 <file>` (Mac) or `Get-FileHash <file>` (Windows).

Or build it yourself on the computer you are testing (no warnings, because nothing was downloaded):

```bash
npm ci
npm run fetch-osquery
npm run build:test          # WTF Helper Test
npm run build               # WTF Helper
```

## Opening an unsigned build

**Mac, downloaded from another computer:**

- macOS 12–14: Control-click the app, choose **Open**, then **Open** again.
- macOS 15 and later: open it once (it is blocked), then go to **System Settings → Privacy & Security**, scroll down, and click **Open Anyway**.

**Windows:** SmartScreen shows "Windows protected your PC". Click **More info**, then **Run anyway**. Some antivirus programs may also warn.

The first time it saves the connection, the Mac may ask for permission to use the Keychain. Choose **Always Allow**. An unsigned build may ask again after each update. A signed build counts as a different app to the Keychain, so testers pair again after switching to it.

## What to try

1. Turn on `WTF_HELPER_ENABLED=true` and `WTF_HELPER_DEPLOYMENT=dev` on the apidev backend, and use a paid-plan account on appdev.
2. Get a pairing code from appdev and type it into WTF Helper Test. The window should show the service, the end of your account ID, and this computer's name.
3. Ask for checks from appdev (the web app cards arrive in 2b-3; until then, call `POST /api/wtf/helper/:deviceId/checks` directly). Each one should appear in the helper's activity log with "You allowed" or "Allow all".
4. Switch on **Pause all checks** and ask again: every check should come back refused, including with Allow all.
5. Turn Wi-Fi off for a minute and back on: the window should show "Reconnecting…" and then "Connected" again.
6. Disconnect this computer from appdev (the web app's Disconnect): the helper should say it was disconnected and go back to the pairing screen.
7. **Disconnect** in the helper: it forgets the connection and returns to the pairing screen.

Please also run every check once on Windows 10 or 11 as a **standard user** (not an administrator) and once on macOS 14 or 15, and note anything that fails in an issue. Those are the combinations CI cannot cover.
