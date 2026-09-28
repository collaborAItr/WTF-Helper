//! Summary checks: one short answer each, sent to the guide straight after approval.

use std::collections::BTreeSet;

use futures_util::join;
use serde_json::{json, Value};

use super::queries::{self, Platform};
use super::{field, field_i64, field_u64, gb, Machine, Outcome, Row, QUERY_FAILED};

fn first(rows: &[Row]) -> Option<&Row> {
    rows.first()
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

pub async fn system_overview<M: Machine>(m: &M) -> Result<Outcome, String> {
    let platform = m.platform();
    let battery_query = match platform {
        Platform::Mac => queries::MAC_BATTERY,
        Platform::Windows => queries::WINDOWS_BATTERY,
    };
    let (system_query, os_query, uptime_query) =
        (queries::SYSTEM_INFO, queries::OS_VERSION, queries::UPTIME);
    let (system, os, uptime, battery_rows) = join!(
        m.query(&system_query),
        m.query(&os_query),
        m.query(&uptime_query),
        m.query(&battery_query)
    );
    let system = system.map_err(|_| QUERY_FAILED.to_string())?;
    let os = os.map_err(|_| QUERY_FAILED.to_string())?;
    let uptime = uptime.unwrap_or_default();
    let battery_rows = battery_rows.unwrap_or_default();

    let sys = first(&system).ok_or(QUERY_FAILED)?;
    let os = first(&os).ok_or(QUERY_FAILED)?;
    let model = match platform {
        Platform::Mac => field(sys, "hardware_model").to_string(),
        Platform::Windows => format!(
            "{} {}",
            field(sys, "hardware_vendor"),
            field(sys, "hardware_model")
        )
        .trim()
        .to_string(),
    };
    let memory_gb = field_u64(sys, "physical_memory").map(gb).map(|g| g.round());
    let cores = field_u64(sys, "cpu_logical_cores");
    let up = first(&uptime);
    let days = up.and_then(|r| field_u64(r, "days"));
    let hours = up.and_then(|r| field_u64(r, "hours"));

    let battery = first(&battery_rows).map(|b| {
        let percent = field_u64(b, "percent_remaining");
        let cycles = field_u64(b, "cycle_count");
        let charging = field(b, "charging") == "1";
        let on_power = field(b, "state") == "AC Power";
        match platform {
            Platform::Mac => json!({
                "percent": percent,
                "health": field(b, "health"),
                "condition": field(b, "condition"),
                "cycleCount": cycles,
                "charging": charging,
                "onPower": on_power,
            }),
            Platform::Windows => {
                let designed = field_u64(b, "designed_capacity").filter(|d| *d > 0);
                let max = field_u64(b, "max_capacity");
                let health = designed
                    .zip(max)
                    .map(|(d, m)| ((m as f64 / d as f64) * 100.0).round() as u64);
                json!({
                    "percent": percent,
                    "healthPercent": health,
                    "cycleCount": cycles,
                    "charging": charging,
                    "onPower": on_power,
                })
            }
        }
    });

    let os_name = field(os, "name");
    let os_version = field(os, "version");
    let mut summary = format!("{os_name} {os_version}");
    if !model.is_empty() {
        summary.push_str(&format!(" on {model}"));
    }
    summary.push_str(&format!(". {}", field(sys, "cpu_brand")));
    if let Some(c) = cores {
        summary.push_str(&format!(", {c} cores"));
    }
    if let Some(g) = memory_gb {
        summary.push_str(&format!(", {g} GB memory"));
    }
    summary.push('.');
    if let (Some(d), Some(h)) = (days, hours) {
        summary.push_str(&format!(" On for {d} days {h} hours."));
    }
    if let Some(b) = &battery {
        if let Some(p) = b["percent"].as_u64() {
            summary.push_str(&format!(" Battery {p}%"));
            if let Some(h) = b
                .get("health")
                .and_then(Value::as_str)
                .filter(|h| !h.is_empty())
            {
                summary.push_str(&format!(", health {h}"));
            }
            if let Some(h) = b.get("healthPercent").and_then(Value::as_u64) {
                summary.push_str(&format!(", holds {h}% of its original charge"));
            }
            if let Some(c) = b["cycleCount"].as_u64() {
                summary.push_str(&format!(", {c} cycles"));
            }
            summary.push('.');
        }
    }

    Ok(Outcome {
        summary,
        data: json!({
            "os": { "name": os_name, "version": os_version, "build": field(os, "build") },
            "model": model,
            "cpu": field(sys, "cpu_brand"),
            "cpuCores": cores,
            "memoryGb": memory_gb,
            "uptime": { "days": days, "hours": hours },
            "battery": battery,
        }),
    })
}

/// `/dev/disk3s1s1` → `disk3`. APFS volumes in one container share the same free space.
fn apfs_container(device: &str) -> Option<&str> {
    let rest = device.strip_prefix("/dev/")?;
    let digits = rest.strip_prefix("disk")?;
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    if end == 0 {
        return None;
    }
    Some(&rest[..4 + end])
}

struct Drive {
    name: String,
    total: u64,
    free: u64,
    file_system: String,
}

fn drive_json(drives: &[Drive]) -> (String, Value) {
    let mut parts = Vec::new();
    let items: Vec<Value> = drives
        .iter()
        .map(|d| {
            let percent = if d.total > 0 {
                (d.free as f64 / d.total as f64 * 100.0).round() as u64
            } else {
                0
            };
            let mut line = format!(
                "{}: {} GB free of {} GB ({percent}%)",
                d.name,
                gb(d.free),
                gb(d.total)
            );
            if percent < 10 {
                line.push_str(", nearly full");
            }
            parts.push(line);
            json!({
                "name": d.name,
                "totalGb": gb(d.total),
                "freeGb": gb(d.free),
                "freePercent": percent,
                "fileSystem": d.file_system,
            })
        })
        .collect();
    let summary = if parts.is_empty() {
        "No drives were found.".to_string()
    } else {
        format!("{}.", parts.join(". "))
    };
    (summary, json!({ "drives": items }))
}

pub async fn disk_space<M: Machine>(m: &M) -> Result<Outcome, String> {
    let mut drives = Vec::new();
    match m.platform() {
        Platform::Mac => {
            let rows = m
                .query(&queries::MAC_MOUNTS)
                .await
                .map_err(|_| QUERY_FAILED.to_string())?;
            let mut seen = BTreeSet::new();
            let mut rows: Vec<&Row> = rows.iter().collect();
            rows.sort_by_key(|r| field(r, "path") != "/");
            for row in rows {
                let Some(container) = apfs_container(field(row, "device")) else {
                    continue;
                };
                if !seen.insert(container.to_string()) {
                    continue;
                }
                let size = field_u64(row, "blocks_size").unwrap_or(0);
                let path = field(row, "path");
                let name = if path == "/" {
                    "Main drive".to_string()
                } else {
                    path.trim_start_matches("/Volumes/").to_string()
                };
                drives.push(Drive {
                    name,
                    total: field_u64(row, "blocks").unwrap_or(0) * size,
                    free: field_u64(row, "blocks_available").unwrap_or(0) * size,
                    file_system: field(row, "type").to_string(),
                });
            }
        }
        Platform::Windows => {
            let rows = m
                .query(&queries::WINDOWS_DRIVES)
                .await
                .map_err(|_| QUERY_FAILED.to_string())?;
            for row in &rows {
                let Some(total) = field_i64(row, "size").filter(|s| *s > 0) else {
                    continue;
                };
                if field(row, "description")
                    .to_ascii_lowercase()
                    .contains("cd-rom")
                {
                    continue;
                }
                let free = field_i64(row, "free_space").unwrap_or(0).max(0);
                let mut name = field(row, "device_id").to_string();
                if field(row, "boot_partition") == "1" {
                    name.push_str(" (Windows drive)");
                }
                drives.push(Drive {
                    name,
                    total: total as u64,
                    free: free as u64,
                    file_system: field(row, "file_system").to_string(),
                });
            }
        }
    }
    let (summary, data) = drive_json(&drives);
    Ok(Outcome { summary, data })
}

fn split_addresses(text: &str) -> Vec<String> {
    text.split(|c: char| {
        c == ','
            || c == ';'
            || c.is_whitespace()
            || c == '{'
            || c == '}'
            || c == '"'
            || c == '['
            || c == ']'
    })
    .filter(|t| !t.is_empty() && (t.contains('.') || t.contains(':')))
    .map(str::to_string)
    .collect()
}

pub async fn network_status<M: Machine>(m: &M) -> Result<Outcome, String> {
    let routes = m.query(&queries::DEFAULT_ROUTE).await.unwrap_or_default();
    let gateways: BTreeSet<String> = routes
        .iter()
        .map(|r| field(r, "gateway").to_string())
        .filter(|g| !g.is_empty())
        .collect();
    let mut active: Vec<String> = Vec::new();
    let mut dns: BTreeSet<String> = BTreeSet::new();
    match m.platform() {
        Platform::Mac => {
            let default_ifaces: BTreeSet<&str> =
                routes.iter().map(|r| field(r, "interface")).collect();
            let rows = m
                .query(&queries::MAC_INTERFACES)
                .await
                .map_err(|_| QUERY_FAILED.to_string())?;
            let up: Vec<&str> = rows
                .iter()
                .filter(|row| {
                    let flags = field_u64(row, "flags").unwrap_or(0);
                    flags & 0x1 != 0 && flags & 0x40 != 0 && flags & 0x8 == 0
                })
                .map(|row| field(row, "interface"))
                .collect();
            // The interface carrying the default route is the real connection; bridge ports also show as up.
            active = up
                .iter()
                .filter(|name| default_ifaces.contains(*name))
                .map(|n| n.to_string())
                .collect();
            if active.is_empty() {
                active = up
                    .iter()
                    .filter(|name| name.starts_with("en"))
                    .map(|n| n.to_string())
                    .collect();
            }
            for row in m.query(&queries::MAC_DNS).await.unwrap_or_default() {
                let address = field(&row, "address");
                if !address.is_empty() {
                    dns.insert(address.to_string());
                }
            }
        }
        Platform::Windows => {
            let rows = m
                .query(&queries::WINDOWS_INTERFACES)
                .await
                .map_err(|_| QUERY_FAILED.to_string())?;
            for row in &rows {
                let status = field(row, "connection_status");
                let connected = status == "2" || status.eq_ignore_ascii_case("connected");
                if connected
                    && field(row, "enabled") != "0"
                    && field(row, "physical_adapter") == "1"
                {
                    active.push(field(row, "friendly_name").to_string());
                    dns.extend(split_addresses(field(row, "dns_server_search_order")));
                }
            }
        }
    }
    let reach = m.reach_service().await;
    let host = m.service_host().to_string();

    let mut summary = if active.is_empty() {
        "No active network connection was found".to_string()
    } else {
        format!("Connected ({})", active.join(", "))
    };
    summary.push_str(if gateways.is_empty() {
        ". No router found"
    } else {
        ". Router found"
    });
    if dns.is_empty() {
        summary.push_str(". No DNS servers set");
    } else {
        summary.push_str(&format!(
            ". DNS: {}",
            dns.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    match (reach.reachable, reach.ms) {
        (true, Some(ms)) => summary.push_str(&format!(". collaborAItr answered in {ms} ms.")),
        (true, None) => summary.push_str(". collaborAItr answered."),
        _ => summary.push_str(". collaborAItr could not be reached."),
    }

    Ok(Outcome {
        summary,
        data: json!({
            "activeConnections": active,
            "routerFound": !gateways.is_empty(),
            "router": gateways.iter().next(),
            "dnsServers": dns,
            "service": { "host": host, "reachable": reach.reachable, "ms": reach.ms, "status": reach.status },
        }),
    })
}

pub async fn security_status<M: Machine>(m: &M) -> Result<Outcome, String> {
    match m.platform() {
        Platform::Mac => {
            let (fv, gk, fw, sp) = (
                queries::MAC_FILEVAULT,
                queries::MAC_GATEKEEPER,
                queries::MAC_FIREWALL,
                queries::MAC_SIP,
            );
            let (filevault, gatekeeper, firewall, sip) =
                join!(m.query(&fv), m.query(&gk), m.query(&fw), m.query(&sp));
            if filevault.is_err() && gatekeeper.is_err() && firewall.is_err() && sip.is_err() {
                return Err(QUERY_FAILED.to_string());
            }
            let filevault = filevault.ok().filter(|r| !r.is_empty()).map(|rows| {
                let data_volume = rows
                    .iter()
                    .find(|r| field(r, "path") == "/System/Volumes/Data")
                    .or(rows.first());
                data_volume
                    .map(|r| field(r, "encrypted") == "1")
                    .unwrap_or(false)
            });
            let gatekeeper = gatekeeper
                .ok()
                .as_deref()
                .and_then(first)
                .map(|r| field(r, "assessments_enabled") == "1");
            let fw_row = firewall.ok().and_then(|rows| rows.into_iter().next());
            let firewall = fw_row.as_ref().map(|r| match field(r, "global_state") {
                "1" => "on",
                "2" => "block_all",
                _ => "off",
            });
            let stealth = fw_row.as_ref().map(|r| field(r, "stealth_enabled") == "1");
            let sip = sip
                .ok()
                .as_deref()
                .and_then(first)
                .map(|r| field(r, "enabled") == "1");

            let describe = |label: &str, value: Option<&str>| match value {
                Some(v) => format!("{label} {v}"),
                None => format!("{label} unknown"),
            };
            let summary = format!(
                "{}. {}. {}. {}.",
                describe("FileVault disk encryption", filevault.map(on_off)),
                describe("Gatekeeper", gatekeeper.map(on_off)),
                describe(
                    "Firewall",
                    firewall.map(|f| if f == "block_all" {
                        "on, blocking all incoming connections"
                    } else {
                        f
                    })
                ),
                describe("System Integrity Protection", sip.map(on_off)),
            );
            Ok(Outcome {
                summary,
                data: json!({
                    "fileVault": filevault.map(on_off),
                    "gatekeeper": gatekeeper.map(on_off),
                    "firewall": firewall,
                    "stealthMode": stealth.map(on_off),
                    "systemIntegrityProtection": sip.map(on_off),
                }),
            })
        }
        Platform::Windows => {
            let center = m
                .query(&queries::WINDOWS_SECURITY_CENTER)
                .await
                .map_err(|_| QUERY_FAILED.to_string())?;
            let center = first(&center).cloned().unwrap_or_default();
            let bitlocker_rows = m
                .query(&queries::WINDOWS_BITLOCKER)
                .await
                .unwrap_or_default();
            // Without administrator rights bitlocker_info returns no rows rather than an error.
            let bitlocker: Value = if bitlocker_rows.is_empty() {
                json!("needs_administrator")
            } else {
                Value::Array(
                    bitlocker_rows
                        .iter()
                        .map(|r| json!({ "drive": field(r, "drive_letter"), "protection": on_off(field(r, "protection_status") == "1") }))
                        .collect(),
                )
            };
            let health = |key: &str| {
                let v = field(&center, key);
                if v.is_empty() {
                    "Unknown".to_string()
                } else {
                    v.to_string()
                }
            };
            let mut summary = format!(
                "Antivirus: {}. Firewall: {}. Windows Update: {}. User Account Control: {}.",
                health("antivirus"),
                health("firewall"),
                health("autoupdate"),
                health("user_account_control"),
            );
            match &bitlocker {
                Value::Array(drives) => {
                    let parts: Vec<String> = drives
                        .iter()
                        .map(|d| {
                            format!(
                                "{} {}",
                                d["drive"].as_str().unwrap_or(""),
                                d["protection"].as_str().unwrap_or("")
                            )
                        })
                        .collect();
                    summary.push_str(&format!(" BitLocker: {}.", parts.join(", ")));
                }
                _ => summary.push_str(
                    " BitLocker status needs administrator rights, so it was not checked.",
                ),
            }
            Ok(Outcome {
                summary,
                data: json!({
                    "antivirus": health("antivirus"),
                    "firewall": health("firewall"),
                    "windowsUpdate": health("autoupdate"),
                    "userAccountControl": health("user_account_control"),
                    "bitlocker": bitlocker,
                }),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::*;
    use super::super::queries::*;
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn mac_overview_leaves_out_the_computer_name() {
        let m = FakeMachine::new(Platform::Mac)
            .answer(
                &SYSTEM_INFO,
                vec![row(&[
                    ("computer_name", "Pat's MacBook"),
                    ("hardware_model", "MacBookPro11,4"),
                    ("cpu_brand", "Intel(R) Core(TM) i7"),
                    ("cpu_logical_cores", "8"),
                    ("physical_memory", "17179869184"),
                ])],
            )
            .answer(
                &OS_VERSION,
                vec![row(&[
                    ("name", "macOS"),
                    ("version", "12.7.3"),
                    ("build", "21H1015"),
                ])],
            )
            .answer(&UPTIME, vec![row(&[("days", "0"), ("hours", "6")])])
            .answer(
                &MAC_BATTERY,
                vec![row(&[
                    ("percent_remaining", "100"),
                    ("health", "Good"),
                    ("condition", ""),
                    ("cycle_count", "366"),
                    ("charging", "0"),
                    ("state", "AC Power"),
                ])],
            );
        let out = system_overview(&m).await.unwrap();
        assert_eq!(
            out.summary,
            "macOS 12.7.3 on MacBookPro11,4. Intel(R) Core(TM) i7, 8 cores, 16 GB memory. On for 0 days 6 hours. Battery 100%, health Good, 366 cycles."
        );
        assert_eq!(out.data["memoryGb"], json!(16.0));
        assert_eq!(out.data["battery"]["onPower"], json!(true));
        assert!(!out.data.to_string().contains("Pat"));
    }

    #[tokio::test]
    async fn windows_overview_reports_battery_health_as_a_percentage() {
        let m = FakeMachine::new(Platform::Windows)
            .answer(
                &SYSTEM_INFO,
                vec![row(&[
                    ("hardware_vendor", "HP"),
                    ("hardware_model", "EliteBook 840"),
                    ("cpu_brand", "Intel i5"),
                    ("cpu_logical_cores", "4"),
                    ("physical_memory", "8589934592"),
                ])],
            )
            .answer(
                &OS_VERSION,
                vec![row(&[
                    ("name", "Microsoft Windows 11 Home"),
                    ("version", "10.0.22631"),
                    ("build", "22631"),
                ])],
            )
            .answer(&UPTIME, vec![])
            .answer(
                &WINDOWS_BATTERY,
                vec![row(&[
                    ("percent_remaining", "55"),
                    ("designed_capacity", "50000"),
                    ("max_capacity", "40000"),
                    ("cycle_count", "300"),
                    ("charging", "1"),
                    ("state", "AC Power"),
                ])],
            );
        let out = system_overview(&m).await.unwrap();
        assert_eq!(out.data["model"], json!("HP EliteBook 840"));
        assert_eq!(out.data["battery"]["healthPercent"], json!(80));
        assert!(out.summary.contains("holds 80% of its original charge"));
    }

    #[tokio::test]
    async fn overview_fails_without_system_info() {
        let m = FakeMachine::new(Platform::Mac);
        assert_eq!(
            system_overview(&m).await.err().as_deref(),
            Some(QUERY_FAILED)
        );
    }

    #[tokio::test]
    async fn mac_disk_space_counts_each_apfs_container_once() {
        let mount = |path: &str, device: &str, fs: &str, size: &str, blocks: &str, free: &str| {
            row(&[
                ("path", path),
                ("device", device),
                ("type", fs),
                ("blocks_size", size),
                ("blocks", blocks),
                ("blocks_available", free),
            ])
        };
        let m = FakeMachine::new(Platform::Mac).answer(
            &MAC_MOUNTS,
            vec![
                mount(
                    "/Volumes/Work",
                    "/dev/disk1s10",
                    "apfs",
                    "4096",
                    "61228134",
                    "10370676",
                ),
                mount(
                    "/",
                    "/dev/disk1s6s1",
                    "apfs",
                    "4096",
                    "61228134",
                    "10370676",
                ),
                mount(
                    "/Volumes/ASHLAND-A",
                    "/dev/disk2s2",
                    "exfat",
                    "262144",
                    "19075908",
                    "14144443",
                ),
                mount(
                    "/Volumes/share",
                    "//pat@nas/share",
                    "smbfs",
                    "4096",
                    "100",
                    "50",
                ),
            ],
        );
        let out = disk_space(&m).await.unwrap();
        let drives = out.data["drives"].as_array().unwrap();
        assert_eq!(drives.len(), 2);
        assert_eq!(drives[0]["name"], json!("Main drive"));
        assert_eq!(drives[0]["freePercent"], json!(17));
        assert_eq!(drives[1]["name"], json!("ASHLAND-A"));
        assert!(out
            .summary
            .starts_with("Main drive: 39.6 GB free of 233.6 GB (17%)."));
    }

    #[tokio::test]
    async fn windows_disk_space_skips_cd_drives_and_flags_nearly_full() {
        let m = FakeMachine::new(Platform::Windows).answer(
            &WINDOWS_DRIVES,
            vec![
                row(&[
                    ("device_id", "C:"),
                    ("description", "Local Fixed Disk"),
                    ("file_system", "NTFS"),
                    ("size", "256060514304"),
                    ("free_space", "12884901888"),
                    ("boot_partition", "1"),
                ]),
                row(&[
                    ("device_id", "D:"),
                    ("description", "CD-ROM Disc"),
                    ("file_system", ""),
                    ("size", "-1"),
                    ("free_space", "-1"),
                    ("boot_partition", "0"),
                ]),
            ],
        );
        let out = disk_space(&m).await.unwrap();
        assert_eq!(out.data["drives"].as_array().unwrap().len(), 1);
        assert!(out
            .summary
            .contains("C: (Windows drive): 12 GB free of 238.5 GB (5%), nearly full."));
    }

    #[tokio::test]
    async fn mac_network_lists_active_interfaces_and_reachability() {
        let m = FakeMachine::new(Platform::Mac)
            .answer(
                &DEFAULT_ROUTE,
                vec![row(&[("gateway", "192.168.1.1"), ("interface", "en0")])],
            )
            .answer(
                &MAC_INTERFACES,
                vec![
                    row(&[("interface", "lo0"), ("flags", "32841")]),
                    row(&[("interface", "en0"), ("flags", "34915")]),
                    row(&[("interface", "en1"), ("flags", "35171")]),
                    row(&[("interface", "awdl0"), ("flags", "35139")]),
                    row(&[("interface", "en5"), ("flags", "0")]),
                ],
            )
            .answer(
                &MAC_DNS,
                vec![row(&[("address", "192.168.1.1")]), row(&[("address", "")])],
            );
        let out = network_status(&m).await.unwrap();
        assert_eq!(out.data["activeConnections"], json!(["en0"]));
        assert_eq!(out.data["routerFound"], json!(true));
        assert_eq!(out.data["dnsServers"], json!(["192.168.1.1"]));
        assert_eq!(
            out.summary,
            "Connected (en0). Router found. DNS: 192.168.1.1. collaborAItr answered in 42 ms."
        );
    }

    #[tokio::test]
    async fn windows_network_reads_dns_from_active_adapters() {
        let mut m = FakeMachine::new(Platform::Windows)
            .answer(&DEFAULT_ROUTE, vec![])
            .answer(
                &WINDOWS_INTERFACES,
                vec![
                    row(&[
                        ("friendly_name", "Wi-Fi"),
                        ("connection_status", "2"),
                        ("enabled", "1"),
                        ("physical_adapter", "1"),
                        ("dns_server_search_order", "1.1.1.1, 8.8.8.8"),
                    ]),
                    row(&[
                        ("friendly_name", "Ethernet"),
                        ("connection_status", "7"),
                        ("enabled", "1"),
                        ("physical_adapter", "1"),
                        ("dns_server_search_order", "9.9.9.9"),
                    ]),
                ],
            );
        m.reach = super::super::Reach {
            reachable: false,
            ms: None,
            status: None,
        };
        let out = network_status(&m).await.unwrap();
        assert_eq!(out.data["activeConnections"], json!(["Wi-Fi"]));
        assert_eq!(out.data["dnsServers"], json!(["1.1.1.1", "8.8.8.8"]));
        assert!(out.summary.ends_with(
            "No router found. DNS: 1.1.1.1, 8.8.8.8. collaborAItr could not be reached."
        ));
    }

    #[tokio::test]
    async fn mac_security_reads_all_four_settings() {
        let m = FakeMachine::new(Platform::Mac)
            .answer(
                &MAC_FILEVAULT,
                vec![
                    row(&[("path", "/"), ("encrypted", "0")]),
                    row(&[("path", "/System/Volumes/Data"), ("encrypted", "1")]),
                ],
            )
            .answer(&MAC_GATEKEEPER, vec![row(&[("assessments_enabled", "1")])])
            .answer(
                &MAC_FIREWALL,
                vec![row(&[("global_state", "0"), ("stealth_enabled", "0")])],
            )
            .answer(&MAC_SIP, vec![row(&[("enabled", "1")])]);
        let out = security_status(&m).await.unwrap();
        assert_eq!(out.data["fileVault"], json!("on"));
        assert_eq!(out.data["firewall"], json!("off"));
        assert_eq!(
            out.summary,
            "FileVault disk encryption on. Gatekeeper on. Firewall off. System Integrity Protection on."
        );
    }

    #[tokio::test]
    async fn windows_security_says_when_bitlocker_needs_an_administrator() {
        let m = FakeMachine::new(Platform::Windows)
            .answer(
                &WINDOWS_SECURITY_CENTER,
                vec![row(&[
                    ("firewall", "Good"),
                    ("antivirus", "Good"),
                    ("autoupdate", "Good"),
                    ("user_account_control", "Poor"),
                ])],
            )
            .answer(&WINDOWS_BITLOCKER, vec![]);
        let out = security_status(&m).await.unwrap();
        assert_eq!(out.data["bitlocker"], json!("needs_administrator"));
        assert!(out
            .summary
            .ends_with("BitLocker status needs administrator rights, so it was not checked."));
    }
}
