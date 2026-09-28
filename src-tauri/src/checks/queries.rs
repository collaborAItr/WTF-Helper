//! Every query the helper can run. Nothing here is built from request text; the only
//! value ever placed into a query is the bounded `windowDays` number on Windows.
//! CHECKS.md at the repo root lists these same strings, and a test keeps them equal.

use std::borrow::Cow;

use crate::protocol::CheckName;

/// A query the helper is allowed to run. Built only from the constants in this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedQuery(Cow<'static, str>);

impl FixedQuery {
    const fn fixed(sql: &'static str) -> Self {
        FixedQuery(Cow::Borrowed(sql))
    }

    pub fn sql(&self) -> &str {
        &self.0
    }

    /// Windows Application log errors for the last `days` days (clamped to 1–30).
    pub fn windows_app_errors(days: u32) -> Self {
        let window_ms = u64::from(days.clamp(1, 30)) * 86_400_000;
        FixedQuery(Cow::Owned(
            WINDOWS_APP_ERRORS_TEMPLATE.replace("{window_ms}", &window_ms.to_string()),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Mac
        }
    }

    pub fn os_name(self) -> &'static str {
        match self {
            Platform::Mac => "macOS",
            Platform::Windows => "Windows",
        }
    }
}

// Both platforms
pub const SYSTEM_INFO: FixedQuery =
    FixedQuery::fixed("SELECT computer_name, hardware_vendor, hardware_model, cpu_brand, cpu_logical_cores, physical_memory FROM system_info;");
pub const OS_VERSION: FixedQuery =
    FixedQuery::fixed("SELECT name, version, build FROM os_version;");
pub const UPTIME: FixedQuery = FixedQuery::fixed("SELECT days, hours FROM uptime;");
pub const DEFAULT_ROUTE: FixedQuery = FixedQuery::fixed(
    "SELECT gateway, interface FROM routes WHERE destination = '0.0.0.0' AND netmask = 0;",
);
pub const PROCESSES: FixedQuery =
    FixedQuery::fixed("SELECT pid, name, resident_size, user_time, system_time FROM processes;");
pub const CPU_CORES: FixedQuery = FixedQuery::fixed("SELECT cpu_logical_cores FROM system_info;");
pub const STARTUP_ITEMS: FixedQuery =
    FixedQuery::fixed("SELECT name, type, source, status FROM startup_items;");

// macOS
pub const MAC_BATTERY: FixedQuery = FixedQuery::fixed(
    "SELECT percent_remaining, health, condition, cycle_count, charging, state FROM battery;",
);
pub const MAC_MOUNTS: FixedQuery = FixedQuery::fixed(
    "SELECT path, device, type, blocks_size, blocks, blocks_available FROM mounts WHERE path = '/' OR path LIKE '/Volumes/%';",
);
pub const MAC_INTERFACES: FixedQuery =
    FixedQuery::fixed("SELECT interface, flags FROM interface_details;");
pub const MAC_DNS: FixedQuery =
    FixedQuery::fixed("SELECT address FROM dns_resolvers WHERE type = 'nameserver';");
pub const MAC_FILEVAULT: FixedQuery = FixedQuery::fixed(
    "SELECT m.path, d.encrypted FROM mounts m JOIN disk_encryption d ON d.name = m.device WHERE m.path IN ('/', '/System/Volumes/Data');",
);
pub const MAC_GATEKEEPER: FixedQuery =
    FixedQuery::fixed("SELECT assessments_enabled FROM gatekeeper;");
pub const MAC_FIREWALL: FixedQuery =
    FixedQuery::fixed("SELECT global_state, stealth_enabled FROM alf;");
pub const MAC_SIP: FixedQuery =
    FixedQuery::fixed("SELECT enabled FROM sip_config WHERE config_flag = 'sip';");
pub const MAC_LAUNCH_AGENTS: FixedQuery = FixedQuery::fixed(
    "SELECT label, path FROM launchd WHERE run_at_load = '1' AND (path LIKE '/Library/LaunchAgents/%' OR path LIKE '/Users/%/Library/LaunchAgents/%');",
);
pub const MAC_CRASHES: FixedQuery =
    FixedQuery::fixed("SELECT identifier, datetime, crash_path FROM crashes;");
pub const MAC_APPS: FixedQuery = FixedQuery::fixed(
    "SELECT name, bundle_short_version FROM apps WHERE (path LIKE '/Applications/%' OR path LIKE '/Users/%/Applications/%') AND path NOT LIKE '%.app/%';",
);

// Windows
pub const WINDOWS_BATTERY: FixedQuery = FixedQuery::fixed(
    "SELECT percent_remaining, designed_capacity, max_capacity, cycle_count, charging, state FROM battery;",
);
pub const WINDOWS_DRIVES: FixedQuery = FixedQuery::fixed(
    "SELECT device_id, description, file_system, size, free_space, boot_partition FROM logical_drives;",
);
pub const WINDOWS_INTERFACES: FixedQuery = FixedQuery::fixed(
    "SELECT friendly_name, connection_status, enabled, physical_adapter, dns_server_search_order FROM interface_details;",
);
pub const WINDOWS_SECURITY_CENTER: FixedQuery = FixedQuery::fixed(
    "SELECT firewall, antivirus, autoupdate, user_account_control FROM windows_security_center;",
);
pub const WINDOWS_BITLOCKER: FixedQuery =
    FixedQuery::fixed("SELECT drive_letter, protection_status FROM bitlocker_info;");
pub const WINDOWS_APP_ERRORS_TEMPLATE: &str = "SELECT datetime, provider_name, eventid FROM windows_eventlog WHERE channel = 'Application' AND timestamp = '{window_ms}' AND level IN (1, 2);";
pub const WINDOWS_PROGRAMS: FixedQuery = FixedQuery::fixed("SELECT name, version FROM programs;");

/// Every query a check may run on a platform, for CHECKS.md and the live test.
pub fn queries_for(check: CheckName, platform: Platform) -> Vec<FixedQuery> {
    use CheckName::*;
    use Platform::*;
    match (check, platform) {
        (SystemOverview, Mac) => vec![SYSTEM_INFO, OS_VERSION, UPTIME, MAC_BATTERY],
        (SystemOverview, Windows) => vec![SYSTEM_INFO, OS_VERSION, UPTIME, WINDOWS_BATTERY],
        (DiskSpace, Mac) => vec![MAC_MOUNTS],
        (DiskSpace, Windows) => vec![WINDOWS_DRIVES],
        (NetworkStatus, Mac) => vec![DEFAULT_ROUTE, MAC_INTERFACES, MAC_DNS],
        (NetworkStatus, Windows) => vec![DEFAULT_ROUTE, WINDOWS_INTERFACES],
        (SecurityStatus, Mac) => vec![MAC_FILEVAULT, MAC_GATEKEEPER, MAC_FIREWALL, MAC_SIP],
        (SecurityStatus, Windows) => vec![WINDOWS_SECURITY_CENTER, WINDOWS_BITLOCKER],
        (HeavyPrograms, _) => vec![PROCESSES, CPU_CORES],
        (StartupItems, Mac) => vec![MAC_LAUNCH_AGENTS, STARTUP_ITEMS],
        (StartupItems, Windows) => vec![STARTUP_ITEMS],
        (RecentCrashes, Mac) => vec![MAC_CRASHES],
        (RecentCrashes, Windows) => vec![FixedQuery::windows_app_errors(1)],
        (InstalledApps, Mac) => vec![MAC_APPS],
        (InstalledApps, Windows) => vec![WINDOWS_PROGRAMS],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECKS_DOC: &str = include_str!("../../../CHECKS.md");

    #[test]
    fn checks_doc_lists_every_query_verbatim() {
        for platform in [Platform::Mac, Platform::Windows] {
            for check in CheckName::ALL {
                for query in queries_for(check, platform) {
                    let sql = if query.sql().contains("windows_eventlog") {
                        WINDOWS_APP_ERRORS_TEMPLATE.to_string()
                    } else {
                        query.sql().to_string()
                    };
                    assert!(CHECKS_DOC.contains(&sql), "CHECKS.md is missing: {sql}");
                }
            }
        }
    }

    #[test]
    fn window_days_are_clamped_and_numeric() {
        assert!(FixedQuery::windows_app_errors(1)
            .sql()
            .contains("timestamp = '86400000'"));
        assert!(FixedQuery::windows_app_errors(7)
            .sql()
            .contains("timestamp = '604800000'"));
        assert!(FixedQuery::windows_app_errors(0)
            .sql()
            .contains("timestamp = '86400000'"));
        assert!(FixedQuery::windows_app_errors(99)
            .sql()
            .contains("timestamp = '2592000000'"));
        assert!(!FixedQuery::windows_app_errors(3).sql().contains('{'));
    }
}
