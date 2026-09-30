//! Protocol version 1. Mirrors `protocol/wtf-helper-protocol.ts`; a test keeps them in step.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_CHECKS_PER_REQUEST: usize = 3;
pub const SUMMARY_MAX_CHARS: usize = 500;
pub const ERROR_MAX_CHARS: usize = 200;
/// Matches the guide's tool-result cap, so a result is never clipped again downstream.
pub const DATA_MAX_CHARS: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckName {
    SystemOverview,
    DiskSpace,
    NetworkStatus,
    SecurityStatus,
    HeavyPrograms,
    StartupItems,
    RecentCrashes,
    InstalledApps,
}

pub struct OptionBound {
    pub key: &'static str,
    pub min: u32,
    pub max: u32,
}

impl CheckName {
    pub const ALL: [CheckName; 8] = [
        CheckName::SystemOverview,
        CheckName::DiskSpace,
        CheckName::NetworkStatus,
        CheckName::SecurityStatus,
        CheckName::HeavyPrograms,
        CheckName::StartupItems,
        CheckName::RecentCrashes,
        CheckName::InstalledApps,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CheckName::SystemOverview => "system_overview",
            CheckName::DiskSpace => "disk_space",
            CheckName::NetworkStatus => "network_status",
            CheckName::SecurityStatus => "security_status",
            CheckName::HeavyPrograms => "heavy_programs",
            CheckName::StartupItems => "startup_items",
            CheckName::RecentCrashes => "recent_crashes",
            CheckName::InstalledApps => "installed_apps",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|check| check.as_str() == name)
    }

    pub fn label(self) -> &'static str {
        match self {
            CheckName::SystemOverview => "Computer overview",
            CheckName::DiskSpace => "Disk space",
            CheckName::NetworkStatus => "Network",
            CheckName::SecurityStatus => "Security settings",
            CheckName::HeavyPrograms => "Busy programs",
            CheckName::StartupItems => "Programs that start by themselves",
            CheckName::RecentCrashes => "Recent crashes",
            CheckName::InstalledApps => "Installed apps",
        }
    }

    pub fn is_list(self) -> bool {
        matches!(
            self,
            CheckName::HeavyPrograms
                | CheckName::StartupItems
                | CheckName::RecentCrashes
                | CheckName::InstalledApps
        )
    }

    pub fn option_bounds(self) -> &'static [OptionBound] {
        match self {
            CheckName::SystemOverview
            | CheckName::DiskSpace
            | CheckName::NetworkStatus
            | CheckName::SecurityStatus => &[],
            CheckName::HeavyPrograms => &[OptionBound {
                key: "limit",
                min: 1,
                max: 25,
            }],
            CheckName::StartupItems => &[OptionBound {
                key: "limit",
                min: 1,
                max: 100,
            }],
            CheckName::RecentCrashes => &[
                OptionBound {
                    key: "limit",
                    min: 1,
                    max: 50,
                },
                OptionBound {
                    key: "windowDays",
                    min: 1,
                    max: 30,
                },
            ],
            CheckName::InstalledApps => &[OptionBound {
                key: "limit",
                min: 1,
                max: 200,
            }],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Person,
    AllowAll,
}

impl Approval {
    pub fn parse(value: &Value) -> Option<Self> {
        match value.as_str() {
            Some("person") => Some(Approval::Person),
            Some("allow_all") => Some(Approval::AllowAll),
            _ => None,
        }
    }

    pub fn activity_label(self) -> &'static str {
        match self {
            Approval::Person => "You allowed",
            Approval::AllowAll => "Allow all",
        }
    }
}

/// Bounded whole-number options. Anything else is refused before this is built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckOptions {
    pub limit: Option<u32>,
    pub window_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRequest {
    pub check: CheckName,
    pub options: CheckOptions,
}

/// A check event as it arrives. Fields stay loose so anything malformed is
/// refused by the gate instead of failing to parse.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCheckEvent {
    pub request_id: Option<Value>,
    pub receipt: Option<Value>,
    pub approval: Option<Value>,
    pub checks: Option<Value>,
    pub expires_at: Option<Value>,
}

#[derive(Debug)]
pub enum StreamEvent {
    /// `at` is the service's clock, used to judge `expiresAt` on a computer whose clock is off.
    Heartbeat {
        at: Option<String>,
    },
    Check(RawCheckEvent),
    Other,
}

pub fn parse_stream_data(data: &str) -> Option<StreamEvent> {
    let value: Value = serde_json::from_str(data).ok()?;
    match value.get("type").and_then(Value::as_str)? {
        "heartbeat" => Some(StreamEvent::Heartbeat {
            at: value.get("at").and_then(Value::as_str).map(str::to_string),
        }),
        "check" => serde_json::from_value(value).ok().map(StreamEvent::Check),
        _ => Some(StreamEvent::Other),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckResult {
    pub check: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl CheckResult {
    pub fn success(check: CheckName, summary: String, data: Value) -> Self {
        CheckResult {
            check: check.as_str().to_string(),
            ok: true,
            summary: Some(clip_chars(&summary, SUMMARY_MAX_CHARS)),
            error: None,
            data: Some(data),
        }
    }

    pub fn failure(check: &str, error: &str) -> Self {
        CheckResult {
            check: check.to_string(),
            ok: false,
            summary: None,
            error: Some(clip_chars(error, ERROR_MAX_CHARS)),
            data: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ResultBody<'a> {
    pub receipt: &'a str,
    pub approval: Approval,
    pub results: &'a [CheckResult],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimRequest {
    pub code: String,
    pub device_name: String,
    pub os: String,
    pub os_version: Option<String>,
    pub helper_version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimData {
    pub device_id: String,
    pub device_token: String,
    pub user_id: String,
}

#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    #[serde(default)]
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    #[serde(default)]
    pub recoverable: bool,
}

pub fn clip_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLISHED: &str = include_str!("../../protocol/wtf-helper-protocol.ts");

    fn published_block(start: &str, end: &str) -> &'static str {
        let from = PUBLISHED.find(start).expect("block in protocol file");
        let rest = &PUBLISHED[from..];
        &rest[..rest.find(end).expect("end of block")]
    }

    #[test]
    fn check_list_matches_published_protocol() {
        let block = published_block("export const WTF_HELPER_CHECKS", "] as const");
        let published: Vec<&str> = block
            .split('\'')
            .enumerate()
            .filter(|(i, _)| i % 2 == 1)
            .map(|(_, s)| s)
            .collect();
        let ours: Vec<&str> = CheckName::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(published, ours);
    }

    #[test]
    fn option_bounds_match_published_protocol() {
        let block = published_block("export const WTF_HELPER_OPTION_BOUNDS", "\n};");
        for check in CheckName::ALL {
            let line = block
                .lines()
                .find(|l| l.trim_start().starts_with(&format!("{}:", check.as_str())))
                .unwrap_or_else(|| panic!("{} missing from bounds", check.as_str()));
            for bound in check.option_bounds() {
                let expected = format!(
                    "{}: {{ min: {}, max: {} }}",
                    bound.key, bound.min, bound.max
                );
                assert!(line.contains(&expected), "{line} should contain {expected}");
            }
            assert_eq!(
                line.matches("min:").count(),
                check.option_bounds().len(),
                "{line}"
            );
        }
    }

    #[test]
    fn labels_match_published_protocol() {
        for check in CheckName::ALL {
            let expected = format!("label: '{}'", check.label());
            assert!(PUBLISHED.contains(&expected), "{expected}");
        }
    }

    #[test]
    fn list_checks_match_published_protocol() {
        let block = published_block("export const WTF_HELPER_LIST_CHECKS", "];");
        for check in CheckName::ALL {
            assert_eq!(
                block.contains(&format!("'{}'", check.as_str())),
                check.is_list()
            );
        }
    }

    #[test]
    fn parses_stream_events() {
        match parse_stream_data(r#"{"type":"heartbeat","at":"2026-09-28T07:14:22.546Z"}"#) {
            Some(StreamEvent::Heartbeat { at }) => {
                assert_eq!(at.as_deref(), Some("2026-09-28T07:14:22.546Z"))
            }
            other => panic!("expected heartbeat, got {other:?}"),
        }
        let check = parse_stream_data(
            r#"{"type":"check","requestId":"r1","receipt":"a.b","approval":"person","checks":[{"check":"disk_space"}],"expiresAt":"2026-09-28T07:14:52.546Z"}"#,
        );
        match check {
            Some(StreamEvent::Check(event)) => {
                assert_eq!(event.request_id.unwrap(), "r1");
                assert_eq!(event.approval.unwrap(), "person");
            }
            other => panic!("expected check, got {other:?}"),
        }
        assert!(matches!(
            parse_stream_data(r#"{"type":"end"}"#),
            Some(StreamEvent::Other)
        ));
        assert!(parse_stream_data("[DONE]").is_none());
        assert!(parse_stream_data("not json").is_none());
    }

    #[test]
    fn results_serialize_without_empty_fields() {
        let result = CheckResult::failure("disk_space", "WTF Helper is paused on this computer.");
        assert_eq!(
            serde_json::to_string(&result).unwrap(),
            r#"{"check":"disk_space","ok":false,"error":"WTF Helper is paused on this computer."}"#
        );
    }

    #[test]
    fn clips_by_characters() {
        let long = "é".repeat(600);
        let clipped = clip_chars(&long, SUMMARY_MAX_CHARS);
        assert_eq!(clipped.chars().count(), SUMMARY_MAX_CHARS);
        assert!(clipped.ends_with('…'));
        assert_eq!(clip_chars("short", 10), "short");
    }
}
