//! List checks. Every row has a plain `label` so the web app can show it for unticking.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use super::queries::{self, FixedQuery, Platform};
use super::{field, field_u64, Machine, Outcome, Row, QUERY_FAILED};
use crate::protocol::CheckOptions;
use crate::timeutil::parse_mac_crash_ms;

const SAMPLE_MS: u64 = 1_000;

fn list_data(items: Vec<Value>, total: usize, limit: usize) -> Value {
    let truncated = total > limit;
    let items: Vec<Value> = items.into_iter().take(limit).collect();
    json!({ "items": items, "total": total, "truncated": truncated })
}

fn names_preview(names: &[String], max: usize) -> String {
    let mut shown: Vec<&str> = names.iter().take(max).map(String::as_str).collect();
    if names.len() > max {
        shown.push("and others");
    }
    shown.join(", ")
}

fn fmt_memory(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

struct Usage {
    cpu_ms: u64,
    memory: u64,
    count: u64,
}

pub async fn heavy_programs<M: Machine>(m: &M, options: &CheckOptions) -> Result<Outcome, String> {
    let limit = options.limit.unwrap_or(10) as usize;
    let before = m
        .query(&queries::PROCESSES)
        .await
        .map_err(|_| QUERY_FAILED.to_string())?;
    let started = m.now_ms();
    m.pause(SAMPLE_MS).await;
    let after = m
        .query(&queries::PROCESSES)
        .await
        .map_err(|_| QUERY_FAILED.to_string())?;
    let elapsed = ((m.now_ms() - started).max(0) as u64).max(SAMPLE_MS);
    let cores = m
        .query(&queries::CPU_CORES)
        .await
        .ok()
        .and_then(|rows| rows.first().and_then(|r| field_u64(r, "cpu_logical_cores")))
        .filter(|c| *c > 0)
        .unwrap_or(1);

    let cpu_time =
        |r: &Row| field_u64(r, "user_time").unwrap_or(0) + field_u64(r, "system_time").unwrap_or(0);
    let earlier: HashMap<&str, u64> = before
        .iter()
        .map(|r| (field(r, "pid"), cpu_time(r)))
        .collect();
    let mut by_name: BTreeMap<String, Usage> = BTreeMap::new();
    for row in &after {
        let name = field(row, "name");
        if name.is_empty() {
            continue;
        }
        let delta = earlier
            .get(field(row, "pid"))
            .map(|t| cpu_time(row).saturating_sub(*t))
            .unwrap_or(0);
        let usage = by_name.entry(name.to_string()).or_insert(Usage {
            cpu_ms: 0,
            memory: 0,
            count: 0,
        });
        usage.cpu_ms += delta;
        usage.memory += field_u64(row, "resident_size").unwrap_or(0);
        usage.count += 1;
    }

    let percent =
        |u: &Usage| ((u.cpu_ms as f64 / (elapsed * cores) as f64) * 1000.0).round() / 10.0;
    let mut by_cpu: Vec<(&String, &Usage)> = by_name.iter().collect();
    by_cpu.sort_by(|a, b| {
        b.1.cpu_ms
            .cmp(&a.1.cpu_ms)
            .then(b.1.memory.cmp(&a.1.memory))
    });
    let mut by_mem: Vec<(&String, &Usage)> = by_name.iter().collect();
    by_mem.sort_by_key(|a| std::cmp::Reverse(a.1.memory));

    let mut chosen: Vec<&String> = Vec::new();
    for (name, _) in by_cpu.iter().take(limit).chain(by_mem.iter().take(limit)) {
        if !chosen.contains(name) {
            chosen.push(name);
        }
    }
    chosen.sort_by(|a, b| by_name[*b].memory.cmp(&by_name[*a].memory));

    let items: Vec<Value> = chosen
        .iter()
        .map(|name| {
            let usage = &by_name[*name];
            let memory_mb = usage.memory / 1_048_576;
            let cpu = percent(usage);
            json!({
                "name": name,
                "cpuPercent": cpu,
                "memoryMb": memory_mb,
                "processes": usage.count,
                "label": format!("{name}: {cpu}% processor, {}", fmt_memory(memory_mb)),
            })
        })
        .collect();

    let top_cpu: Vec<String> = by_cpu
        .iter()
        .take(3)
        .filter(|(_, u)| u.cpu_ms > 0)
        .map(|(n, u)| format!("{n} ({}%)", percent(u)))
        .collect();
    let top_mem: Vec<String> = by_mem
        .iter()
        .take(3)
        .map(|(n, u)| format!("{n} ({})", fmt_memory(u.memory / 1_048_576)))
        .collect();
    let summary = format!(
        "Using the most processor: {}. Using the most memory: {}.",
        if top_cpu.is_empty() {
            "nothing busy right now".to_string()
        } else {
            top_cpu.join(", ")
        },
        top_mem.join(", ")
    );
    let total = items.len();
    Ok(Outcome {
        summary,
        data: list_data(items, total, limit * 2),
    })
}

fn windows_startup_kind(source: &str) -> &'static str {
    let lower = source.to_ascii_lowercase();
    if lower.contains("\\run") {
        "Starts at sign-in (registry)"
    } else if lower.contains("startup") {
        "Startup folder"
    } else {
        "Starts at sign-in"
    }
}

fn mac_startup_kind(kind: &str) -> &'static str {
    match kind.to_ascii_lowercase().as_str() {
        "login item" | "app" => "Login item",
        "agent" => "Starts at login (launch agent)",
        "daemon" => "Starts with the computer (launch daemon)",
        _ => "Starts at login",
    }
}

pub async fn startup_items<M: Machine>(m: &M, options: &CheckOptions) -> Result<Outcome, String> {
    let limit = options.limit.unwrap_or(50) as usize;
    let mut entries: BTreeMap<String, Value> = BTreeMap::new();
    let mut add = |name: &str, kind: &str, scope: Option<&str>, enabled: Option<bool>| {
        // Windows keeps a hidden desktop.ini in every Startup folder; it is not a program.
        if name.is_empty() || name.eq_ignore_ascii_case("desktop.ini") {
            return;
        }
        let mut label = format!("{name} ({kind})");
        if enabled == Some(false) {
            label.push_str(", turned off");
        }
        entries.entry(name.to_ascii_lowercase()).or_insert_with(|| {
            json!({ "name": name, "kind": kind, "forAllUsers": scope.map(|s| s == "all"), "enabled": enabled, "label": label })
        });
    };
    let status_enabled = |status: &str| -> Option<bool> {
        let s = status.to_ascii_lowercase();
        if s.is_empty() {
            None
        } else {
            Some(!s.contains("disabled"))
        }
    };

    match m.platform() {
        Platform::Mac => {
            let agents = m.query(&queries::MAC_LAUNCH_AGENTS).await;
            // On macOS 13 and later startup_items needs root; on macOS 12 it is empty. It adds login items when readable.
            let items = m.query(&queries::STARTUP_ITEMS).await.unwrap_or_default();
            let agents = agents.map_err(|_| QUERY_FAILED.to_string())?;
            for row in &agents {
                let scope = if field(row, "path").starts_with("/Library/") {
                    "all"
                } else {
                    "you"
                };
                add(
                    field(row, "label"),
                    "Starts at login (launch agent)",
                    Some(scope),
                    None,
                );
            }
            for row in &items {
                add(
                    field(row, "name"),
                    mac_startup_kind(field(row, "type")),
                    None,
                    status_enabled(field(row, "status")),
                );
            }
        }
        Platform::Windows => {
            let rows = m
                .query(&queries::STARTUP_ITEMS)
                .await
                .map_err(|_| QUERY_FAILED.to_string())?;
            for row in &rows {
                let source = field(row, "source");
                let scope = if source
                    .to_ascii_uppercase()
                    .starts_with("HKEY_LOCAL_MACHINE")
                    || source.to_ascii_lowercase().contains("programdata")
                {
                    "all"
                } else {
                    "you"
                };
                add(
                    field(row, "name"),
                    windows_startup_kind(source),
                    Some(scope),
                    status_enabled(field(row, "status")),
                );
            }
        }
    }

    let items: Vec<Value> = entries.into_values().collect();
    let total = items.len();
    let names: Vec<String> = items
        .iter()
        .filter_map(|v| v["name"].as_str().map(str::to_string))
        .collect();
    let summary = match total {
        0 => "No programs are set to start by themselves.".to_string(),
        1 => format!("1 program starts by itself: {}.", names_preview(&names, 12)),
        n => format!(
            "{n} programs start by themselves: {}.",
            names_preview(&names, 12)
        ),
    };
    Ok(Outcome {
        summary,
        data: list_data(items, total, limit),
    })
}

/// `node-2026-09-26-181006.000.ips` → `node`.
fn crash_process_name(crash_path: &str) -> String {
    let file = crash_path.rsplit(['/', '\\']).next().unwrap_or("");
    let mut stem = file;
    for ext in [".ips", ".crash", ".hang", ".diag"] {
        if let Some(s) = stem.strip_suffix(ext) {
            stem = s;
        }
    }
    if let Some((head, tail)) = stem.rsplit_once('.') {
        if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
            stem = head;
        }
    }
    // Trailing -YYYY-MM-DD-HHMMSS (18 characters)
    if stem.len() > 18 {
        let (head, tail) = stem.split_at(stem.len() - 18);
        let b = tail.as_bytes();
        let shape = b[0] == b'-' && b[5] == b'-' && b[8] == b'-' && b[11] == b'-';
        let digits = tail
            .bytes()
            .enumerate()
            .all(|(i, c)| [0, 5, 8, 11].contains(&i) || c.is_ascii_digit());
        if shape && digits {
            stem = head;
        }
    }
    stem.to_string()
}

fn count_by(names: impl Iterator<Item = String>) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for n in names {
        *counts.entry(n).or_default() += 1;
    }
    let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted
        .iter()
        .take(5)
        .map(|(n, c)| format!("{n} ({c})"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn recent_crashes<M: Machine>(m: &M, options: &CheckOptions) -> Result<Outcome, String> {
    let limit = options.limit.unwrap_or(20) as usize;
    let platform = m.platform();
    let window_days = options.window_days.unwrap_or(match platform {
        Platform::Mac => 7,
        Platform::Windows => 1,
    });
    let mut dated: Vec<(i64, Value, String)> = Vec::new();
    match platform {
        Platform::Mac => {
            let rows = m
                .query(&queries::MAC_CRASHES)
                .await
                .map_err(|_| QUERY_FAILED.to_string())?;
            let cutoff = m.now_ms() - i64::from(window_days) * 86_400_000;
            for row in &rows {
                let Some(at) = parse_mac_crash_ms(field(row, "datetime")) else {
                    continue;
                };
                if at < cutoff {
                    continue;
                }
                let name = crash_process_name(field(row, "crash_path"));
                let time: String = field(row, "datetime").chars().take(16).collect();
                dated.push((
                    at,
                    json!({ "name": name, "identifier": field(row, "identifier"), "time": time, "label": format!("{name}, {time}") }),
                    name,
                ));
            }
        }
        Platform::Windows => {
            let rows = m
                .query(&FixedQuery::windows_app_errors(window_days))
                .await
                .map_err(|_| QUERY_FAILED.to_string())?;
            for row in &rows {
                let raw = field(row, "datetime");
                let time: String = raw.replace('T', " ").chars().take(16).collect();
                let source = field(row, "provider_name").to_string();
                let event = field_u64(row, "eventid");
                let at = crate::timeutil::parse_iso_utc_ms(raw).unwrap_or(0);
                let label = match event {
                    Some(id) => format!("{source} (event {id}), {time} UTC"),
                    None => format!("{source}, {time} UTC"),
                };
                dated.push((at, json!({ "source": source, "eventId": event, "time": format!("{time} UTC"), "label": label }), source));
            }
        }
    }
    dated.sort_by_key(|a| std::cmp::Reverse(a.0));
    let total = dated.len();
    let top = count_by(dated.iter().map(|d| d.2.clone()));
    let period = if window_days == 1 {
        "day".to_string()
    } else {
        format!("{window_days} days")
    };
    let what = match (platform, total == 1) {
        (Platform::Mac, false) => "crash reports",
        (Platform::Mac, true) => "crash report",
        (Platform::Windows, false) => "application errors",
        (Platform::Windows, true) => "application error",
    };
    let summary = if total == 0 {
        format!("No {what} in the last {period}.")
    } else {
        format!("{total} {what} in the last {period}. Most often: {top}.")
    };
    let items: Vec<Value> = dated.into_iter().map(|d| d.1).collect();
    Ok(Outcome {
        summary,
        data: list_data(items, total, limit),
    })
}

pub async fn installed_apps<M: Machine>(m: &M, options: &CheckOptions) -> Result<Outcome, String> {
    let limit = options.limit.unwrap_or(100) as usize;
    let (query, version_key) = match m.platform() {
        Platform::Mac => (queries::MAC_APPS, "bundle_short_version"),
        Platform::Windows => (queries::WINDOWS_PROGRAMS, "version"),
    };
    let rows = m
        .query(&query)
        .await
        .map_err(|_| QUERY_FAILED.to_string())?;
    let mut apps: BTreeMap<(String, String), Value> = BTreeMap::new();
    for row in &rows {
        let name = field(row, "name").trim_end_matches(".app").trim();
        if name.is_empty() {
            continue;
        }
        let version = field(row, version_key);
        let label = if version.is_empty() {
            name.to_string()
        } else {
            format!("{name} {version}")
        };
        apps.entry((name.to_lowercase(), version.to_string()))
            .or_insert_with(|| json!({ "name": name, "version": version, "label": label }));
    }
    let items: Vec<Value> = apps.into_values().collect();
    let total = items.len();
    let summary = if total > limit {
        format!("{total} apps installed. The first {limit} are listed alphabetically.")
    } else {
        format!("{total} apps installed.")
    };
    Ok(Outcome {
        summary,
        data: list_data(items, total, limit),
    })
}

#[cfg(test)]
mod tests {
    use super::super::fake::*;
    use super::super::queries::*;
    use super::*;
    use serde_json::json;

    fn proc(pid: &str, name: &str, mem: &str, user: &str, system: &str) -> Row {
        row(&[
            ("pid", pid),
            ("name", name),
            ("resident_size", mem),
            ("user_time", user),
            ("system_time", system),
        ])
    }

    #[tokio::test]
    async fn heavy_programs_samples_cpu_and_groups_by_name() {
        let m = FakeMachine::new(Platform::Mac)
            .answer(
                &PROCESSES,
                vec![
                    proc("1", "Chrome", "104857600", "1000", "0"),
                    proc("2", "Chrome", "104857600", "0", "0"),
                    proc("3", "node", "1073741824", "5000", "0"),
                ],
            )
            .answer(
                &PROCESSES,
                vec![
                    proc("1", "Chrome", "104857600", "1500", "100"),
                    proc("2", "Chrome", "104857600", "200", "0"),
                    proc("3", "node", "1073741824", "5000", "0"),
                    proc("4", "new", "1048576", "900", "0"),
                ],
            )
            .answer(&CPU_CORES, vec![row(&[("cpu_logical_cores", "4")])]);
        let out = heavy_programs(
            &m,
            &CheckOptions {
                limit: Some(1),
                window_days: None,
            },
        )
        .await
        .unwrap();
        let items = out.data["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], json!("node"));
        assert_eq!(items[0]["memoryMb"], json!(1024));
        assert_eq!(items[1]["name"], json!("Chrome"));
        assert_eq!(items[1]["processes"], json!(2));
        assert_eq!(items[1]["cpuPercent"], json!(20.0));
        assert_eq!(items[1]["label"], json!("Chrome: 20% processor, 200 MB"));
        assert_eq!(out.summary, "Using the most processor: Chrome (20%). Using the most memory: node (1.0 GB), Chrome (200 MB), new (1 MB).");
    }

    #[tokio::test]
    async fn mac_startup_items_use_launch_agents_when_btm_is_unreadable() {
        let m = FakeMachine::new(Platform::Mac)
            .answer(
                &MAC_LAUNCH_AGENTS,
                vec![
                    row(&[
                        ("label", "com.microsoft.update.agent"),
                        (
                            "path",
                            "/Library/LaunchAgents/com.microsoft.update.agent.plist",
                        ),
                    ]),
                    row(&[
                        ("label", "com.spotify.webhelper"),
                        (
                            "path",
                            "/Users/pat/Library/LaunchAgents/com.spotify.webhelper.plist",
                        ),
                    ]),
                ],
            )
            .answer(&STARTUP_ITEMS, vec![]);
        let out = startup_items(&m, &CheckOptions::default()).await.unwrap();
        let items = out.data["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["forAllUsers"], json!(true));
        assert_eq!(items[1]["forAllUsers"], json!(false));
        assert_eq!(
            out.summary,
            "2 programs start by themselves: com.microsoft.update.agent, com.spotify.webhelper."
        );
        assert!(!out.data.to_string().contains("/Users/"));
    }

    #[tokio::test]
    async fn windows_startup_items_describe_their_source() {
        let m = FakeMachine::new(Platform::Windows).answer(
            &STARTUP_ITEMS,
            vec![
                row(&[("name", "OneDrive"), ("type", "Startup Item"), ("source", "HKEY_USERS\\S-1-5-21\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run"), ("status", "enabled")]),
                row(&[("name", "Zoom.lnk"), ("type", "Startup Item"), ("source", "C:\\Users\\pat\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup"), ("status", "disabled")]),
                row(&[("name", "desktop.ini"), ("type", "Startup Item"), ("source", "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\StartUp"), ("status", "enabled")]),
            ],
        );
        let out = startup_items(&m, &CheckOptions::default()).await.unwrap();
        let items = out.data["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["kind"], json!("Starts at sign-in (registry)"));
        assert_eq!(
            items[1]["label"],
            json!("Zoom.lnk (Startup folder), turned off")
        );
    }

    #[test]
    fn crash_names_come_from_the_report_file_name() {
        assert_eq!(
            crash_process_name(
                "/Users/pat/Library/Logs/DiagnosticReports/Retired/node-2026-09-26-181006.000.ips"
            ),
            "node"
        );
        assert_eq!(
            crash_process_name("/Library/Logs/DiagnosticReports/Safari-2026-09-20-101010.ips"),
            "Safari"
        );
        assert_eq!(
            crash_process_name("/x/Google Chrome Helper-2026-01-02-030405.crash"),
            "Google Chrome Helper"
        );
        assert_eq!(crash_process_name("/x/odd.ips"), "odd");
    }

    #[tokio::test]
    async fn mac_crashes_keep_the_window_and_newest_first() {
        let m = FakeMachine::new(Platform::Mac).answer(
            &MAC_CRASHES,
            vec![
                row(&[
                    ("identifier", "com.apple.Safari"),
                    ("datetime", "2026-09-27 09:00:00.0000 -0400"),
                    (
                        "crash_path",
                        "/Users/pat/Library/Logs/DiagnosticReports/Safari-2026-09-27-090000.ips",
                    ),
                ]),
                row(&[
                    ("identifier", "node"),
                    ("datetime", "2026-09-26 18:10:06.0129 -0400"),
                    (
                        "crash_path",
                        "/Users/pat/Library/Logs/DiagnosticReports/node-2026-09-26-181006.ips",
                    ),
                ]),
                row(&[
                    ("identifier", "old"),
                    ("datetime", "2026-08-01 10:00:00.0000 -0400"),
                    ("crash_path", "/x/old-2026-08-01-100000.ips"),
                ]),
            ],
        );
        let out = recent_crashes(&m, &CheckOptions::default()).await.unwrap();
        let items = out.data["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], json!("Safari"));
        assert_eq!(items[0]["label"], json!("Safari, 2026-09-27 09:00"));
        assert_eq!(
            out.summary,
            "2 crash reports in the last 7 days. Most often: Safari (1), node (1)."
        );
    }

    #[tokio::test]
    async fn windows_crashes_use_the_requested_window() {
        let query = FixedQuery::windows_app_errors(3);
        let m = FakeMachine::new(Platform::Windows).answer(
            &query,
            vec![row(&[
                ("datetime", "2026-09-27T10:11:12.1234567Z"),
                ("provider_name", "Application Error"),
                ("eventid", "1000"),
            ])],
        );
        let out = recent_crashes(
            &m,
            &CheckOptions {
                limit: None,
                window_days: Some(3),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            out.data["items"][0]["label"],
            json!("Application Error (event 1000), 2026-09-27 10:11 UTC")
        );
        assert_eq!(
            out.summary,
            "1 application error in the last 3 days. Most often: Application Error (1)."
        );
        assert!(m.asked.lock().unwrap()[0].contains("timestamp = '259200000'"));
    }

    #[tokio::test]
    async fn installed_apps_are_sorted_deduplicated_and_limited() {
        let m = FakeMachine::new(Platform::Mac).answer(
            &MAC_APPS,
            vec![
                row(&[("name", "Zoom.app"), ("bundle_short_version", "6.1")]),
                row(&[("name", "Safari.app"), ("bundle_short_version", "17.6")]),
                row(&[("name", "safari.app"), ("bundle_short_version", "17.6")]),
                row(&[("name", "Arc.app"), ("bundle_short_version", "")]),
            ],
        );
        let out = installed_apps(
            &m,
            &CheckOptions {
                limit: Some(2),
                window_days: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(out.data["total"], json!(3));
        assert_eq!(out.data["truncated"], json!(true));
        assert_eq!(
            out.data["items"],
            json!([{ "name": "Arc", "version": "", "label": "Arc" }, { "name": "Safari", "version": "17.6", "label": "Safari 17.6" }])
        );
        assert_eq!(
            out.summary,
            "3 apps installed. The first 2 are listed alphabetically."
        );
    }
}
