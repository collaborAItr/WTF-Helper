# Checks (v1 allowlist)

WTF Helper runs only these eight checks, and only the queries written here. It runs [osquery](https://osquery.io) `5.23.1` in shell mode, as the signed-in user, never as administrator and never as the root daemon. No query is built from anything the guide or the service sends; the only value ever placed into a query is the bounded `windowDays` number for Windows crashes. A test in `src-tauri/src/checks/queries.rs` fails if any query in the code is missing from this page.

Before anything is sent:

- Only the fields listed under **Sent** leave the computer. Everything else a query returns is dropped.
- The home folder path and the username are replaced with `~`.
- `data` is kept under 8,000 characters; list checks drop rows from the end and set `truncated: true`.
- Raw osquery errors are not sent, because they can contain paths. A failed check says "The computer did not answer this check."

Each list check returns `{ items: [...], total, truncated }`, and every item has a plain `label` (for example "Safari 17.6") so the person can untick rows before sending.

## `system_overview` (summary)

**Computer overview.** The model, system version, processor, memory, how long it has been on, and battery health.

| Mac and Windows |
|---|
| `SELECT computer_name, hardware_vendor, hardware_model, cpu_brand, cpu_logical_cores, physical_memory FROM system_info;` |
| `SELECT name, version, build FROM os_version;` |
| `SELECT days, hours FROM uptime;` |

| Mac | Windows |
|---|---|
| `SELECT percent_remaining, health, condition, cycle_count, charging, state FROM battery;` | `SELECT percent_remaining, designed_capacity, max_capacity, cycle_count, charging, state FROM battery;` |

**Sent:** `os { name, version, build }`, `model`, `cpu`, `cpuCores`, `memoryGb`, `uptime { days, hours }`, `battery` (Mac: `percent`, `health`, `condition`, `cycleCount`, `charging`, `onPower`; Windows: `percent`, `healthPercent` = max ÷ designed capacity, `cycleCount`, `charging`, `onPower`). `computer_name` is read only to name the device when pairing; it is never part of a check result.

## `disk_space` (summary)

**Disk space.** How full your drives are. Only totals, no file names.

| Mac | Windows |
|---|---|
| `SELECT path, device, type, blocks_size, blocks, blocks_available FROM mounts WHERE path = '/' OR path LIKE '/Volumes/%';` | `SELECT device_id, description, file_system, size, free_space, boot_partition FROM logical_drives;` |

**Sent:** `drives[] { name, totalGb, freeGb, freePercent, fileSystem }`. On a Mac, APFS volumes that share a container are counted once, and network shares are skipped. On Windows, CD drives and drives with no size are skipped.

## `network_status` (summary)

**Network.** Whether you are connected, your router and DNS addresses, and whether collaborAItr can be reached. Not your Wi-Fi name.

| Mac and Windows |
|---|
| `SELECT gateway, interface FROM routes WHERE destination = '0.0.0.0' AND netmask = 0;` |

| Mac | Windows |
|---|---|
| `SELECT interface, flags FROM interface_details;` | `SELECT friendly_name, connection_status, enabled, physical_adapter, dns_server_search_order FROM interface_details;` |
| `SELECT address FROM dns_resolvers WHERE type = 'nameserver';` | |

Reachability is a plain `GET /health` on the build's own pinned host (5-second limit), made by the helper itself, not through osquery.

**Sent:** `activeConnections[]` (interface names such as `en0` or `Wi-Fi`), `routerFound`, `router` (its local address), `dnsServers[]`, `service { host, reachable, ms, status }`. No MAC addresses, no Wi-Fi network name (reading it on a Mac needs Location permission).

## `security_status` (summary)

**Security settings.** Whether disk encryption, the firewall and built-in protection are switched on.

| Mac | Windows |
|---|---|
| `SELECT m.path, d.encrypted FROM mounts m JOIN disk_encryption d ON d.name = m.device WHERE m.path IN ('/', '/System/Volumes/Data');` | `SELECT firewall, antivirus, autoupdate, user_account_control FROM windows_security_center;` |
| `SELECT assessments_enabled FROM gatekeeper;` | `SELECT drive_letter, protection_status FROM bitlocker_info;` |
| `SELECT global_state, stealth_enabled FROM alf;` | |
| `SELECT enabled FROM sip_config WHERE config_flag = 'sip';` | |

**Sent:** Mac: `fileVault`, `gatekeeper`, `firewall` (`on` / `off` / `block_all`), `stealthMode`, `systemIntegrityProtection`. Windows: `antivirus`, `firewall`, `windowsUpdate`, `userAccountControl` (the Security Center health words: Good, Poor, Snoozed, Not Monitored, Error), and `bitlocker`: a list of `{ drive, protection }`, or `"needs_administrator"` when the table returns nothing.

## `heavy_programs` (list)

**Busy programs.** The names of the programs using the most processor and memory right now.

| Mac and Windows |
|---|
| `SELECT pid, name, resident_size, user_time, system_time FROM processes;` (run twice, one second apart) |
| `SELECT cpu_logical_cores FROM system_info;` |

Processor use is the change in CPU time over that second, as a share of all cores. Processes with the same name are added together. Options: `limit` 1–25 (default 10) for each ranking; the list holds the top `limit` by processor and the top `limit` by memory.

**Sent:** `items[] { name, cpuPercent, memoryMb, processes, label }`. No paths, command lines or process ids.

## `startup_items` (list)

**Programs that start by themselves.** The names of programs set to start when you sign in.

| Mac | Windows |
|---|---|
| `SELECT label, path FROM launchd WHERE run_at_load = '1' AND (path LIKE '/Library/LaunchAgents/%' OR path LIKE '/Users/%/Library/LaunchAgents/%');` | `SELECT name, type, source, status FROM startup_items;` |
| `SELECT name, type, source, status FROM startup_items;` | |

Options: `limit` 1–100 (default 50).

**Sent:** `items[] { name, kind, forAllUsers, enabled, label }`. The `path` and `source` columns are read only to tell "all users" from "only you" and registry from Startup folder; they are not sent.

## `recent_crashes` (list)

**Recent crashes.** The names and times of programs that crashed recently. Not what was in them.

| Mac | Windows |
|---|---|
| `SELECT identifier, datetime, crash_path FROM crashes;` | `SELECT datetime, provider_name, eventid FROM windows_eventlog WHERE channel = 'Application' AND timestamp = '{window_ms}' AND level IN (1, 2);` |

`{window_ms}` is `windowDays × 86,400,000`, with `windowDays` a whole number from 1 to 30. Options: `limit` 1–50 (default 20), `windowDays` 1–30 (default 7 on a Mac, 1 on Windows). On a Mac the helper keeps reports newer than the window; on Windows the event log filters by time itself. Levels 1 and 2 are Critical and Error.

**Sent:** Mac: `items[] { name, identifier, time, label }`, where `name` comes from the report's file name. Windows: `items[] { source, eventId, time, label }`. The report contents, the event `data` field, and the report path are never read into the result.

## `installed_apps` (list)

**Installed apps.** The names and versions of the apps installed on this computer.

| Mac | Windows |
|---|---|
| `SELECT name, bundle_short_version FROM apps WHERE (path LIKE '/Applications/%' OR path LIKE '/Users/%/Applications/%') AND path NOT LIKE '%.app/%';` | `SELECT name, version FROM programs;` |

Options: `limit` 1–200 (default 100). Sorted by name, duplicates removed.

**Sent:** `items[] { name, version, label }`.

## Table check (osquery 5.23.1)

The spec asked PR 2b-2 to confirm each table exists on current osquery and which need administrator rights.

**macOS**: run by hand on macOS 12.7.3 (Intel) as a standard signed-in user, and on every CI run on GitHub's macOS runner (also a signed-in user, not root). Every table above answered without administrator rights.

| Table | Result |
|---|---|
| `system_info`, `os_version`, `uptime`, `battery` | OK |
| `mounts`, `disk_encryption` | OK. FileVault is read from the `/System/Volumes/Data` volume on APFS. |
| `interface_details`, `routes`, `dns_resolvers` | OK |
| `gatekeeper`, `alf`, `sip_config` | OK. `alf` reads the firewall settings file; on macOS 15 and later Apple moved those settings, so this can report "off" wrongly there. To recheck when a macOS 15 tester is available. |
| `processes` | OK |
| `launchd` | OK |
| `startup_items` | **Substitution.** On macOS 13 and later it reads the Background Task Management database, which needs root; on macOS 12 that database does not exist. It returned no rows here. The Mac check therefore lists LaunchAgents that run at load, from `launchd`, and adds `startup_items` rows only when they can be read. Login items added in System Settings are not visible without root. |
| `crashes` | OK. Reads `.ips` reports in `~/Library/Logs/DiagnosticReports` (including `Retired`) and `/Library/Logs/DiagnosticReports`. |
| `apps` | OK. Filtered to `/Applications` and `~/Applications`, without apps nested inside other apps. |

**Windows**: checked against the osquery 5.23.1 table specs, and by the live query test on GitHub's Windows runner (which runs as administrator, so it cannot show what a standard user is refused).

| Table | Result |
|---|---|
| `system_info`, `os_version`, `uptime`, `battery` | Exist. `battery` on Windows has no `health` or `condition` column, so health is worked out from capacity. |
| `logical_drives` | Exists |
| `interface_details`, `routes` | Exist |
| `dns_resolvers` | **Substitution.** POSIX only. DNS servers come from `interface_details.dns_server_search_order` for connected adapters. |
| `windows_security_center` | Exists. Reports health words, not on/off. |
| `bitlocker_info` | Exists. **Needs administrator**: as a standard user it returns no rows, which the helper reports as "needs administrator" instead of asking for elevation. |
| `processes`, `startup_items` | Exist. `startup_items` covers the `Run` keys and Startup folders. |
| Logon scheduled tasks | **Not covered.** The spec listed "logon tasks"; osquery's `scheduled_tasks` table does not expose triggers, so a logon task cannot be told apart from any other task. Left out of v1. |
| `windows_eventlog` | Exists. Needs `channel`; the `timestamp` constraint filters by age inside the event log. |
| `programs` | Exists |

Still to do by hand: run every check once on Windows 10/11 as a standard (non-administrator) user, and once on macOS 14 or 15.
