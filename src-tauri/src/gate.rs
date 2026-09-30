//! The helper's own gate. The service checks every request first; this refuses
//! anything that slipped past it, and everything while the person has paused.

use serde_json::Value;

use crate::protocol::{
    Approval, CheckName, CheckOptions, CheckRequest, RawCheckEvent, MAX_CHECKS_PER_REQUEST,
};
use crate::timeutil::parse_iso_utc_ms;

/// Longest the helper spends on one request, well inside the service's ~30 second wait.
pub const RUN_BUDGET_MS: i64 = 20_000;
/// Time kept back to post the result before the receipt expires.
pub const POST_MARGIN_MS: i64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Paused,
    UnknownCheck,
    BadOptions,
    TooManyChecks,
    NoApproval,
    Expired,
    Malformed,
}

impl Refusal {
    /// Sent to the guide as the result's `error`.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::Paused => "WTF Helper is paused on this computer.",
            Refusal::UnknownCheck => "WTF Helper does not run that check.",
            Refusal::BadOptions => "WTF Helper refused the options for this check.",
            Refusal::TooManyChecks => "WTF Helper runs at most 3 checks at a time.",
            Refusal::NoApproval => "The check had no approval.",
            Refusal::Expired => "The check arrived too late.",
            Refusal::Malformed => "The check request was not valid.",
        }
    }

    /// Shown in the activity log.
    pub fn log_label(self) -> &'static str {
        match self {
            Refusal::Paused => "Refused: paused",
            Refusal::UnknownCheck => "Refused: unknown check",
            Refusal::BadOptions => "Refused: options not allowed",
            Refusal::TooManyChecks => "Refused: too many checks",
            Refusal::NoApproval => "Refused: no approval",
            Refusal::Expired => "Refused: arrived too late",
            Refusal::Malformed => "Refused: not a valid request",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Run {
        request_id: String,
        receipt: String,
        approval: Approval,
        checks: Vec<CheckRequest>,
        deadline_ms: i64,
    },
    /// Run nothing; post a refusal for every requested name so the web app hears back at once.
    Refuse {
        request_id: String,
        receipt: String,
        approval: Approval,
        names: Vec<String>,
        reason: Refusal,
    },
    /// Run nothing and post nothing: the request cannot be answered safely.
    Drop { names: Vec<String>, reason: Refusal },
}

fn is_safe_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

fn parse_options(check: CheckName, raw: Option<&Value>) -> Option<CheckOptions> {
    let mut options = CheckOptions::default();
    let Some(raw) = raw else { return Some(options) };
    if raw.is_null() {
        return Some(options);
    }
    let map = raw.as_object()?;
    for (key, value) in map {
        let bound = check.option_bounds().iter().find(|b| b.key == key)?;
        let n = value.as_u64()?;
        if n < u64::from(bound.min) || n > u64::from(bound.max) {
            return None;
        }
        match key.as_str() {
            "limit" => options.limit = Some(n as u32),
            "windowDays" => options.window_days = Some(n as u32),
            _ => return None,
        }
    }
    Some(options)
}

pub fn decide(event: &RawCheckEvent, paused: bool, now_ms: i64) -> Decision {
    let items: Vec<&Value> = match event.checks.as_ref().and_then(Value::as_array) {
        Some(list) => list.iter().collect(),
        None => Vec::new(),
    };
    let mut names: Vec<String> = Vec::new();
    let mut malformed = items.is_empty();
    for item in &items {
        match item.get("check").and_then(Value::as_str) {
            Some(name) if is_plain_name(name) && !names.iter().any(|n| n == name) => {
                names.push(name.to_string())
            }
            _ => malformed = true,
        }
    }
    let drop = |reason: Refusal, names: Vec<String>| Decision::Drop { names, reason };

    let request_id = match event.request_id.as_ref().and_then(Value::as_str) {
        Some(id) if is_safe_request_id(id) => id.to_string(),
        _ => return drop(Refusal::Malformed, names),
    };
    let receipt = match event.receipt.as_ref().and_then(Value::as_str) {
        Some(r) if !r.is_empty() && r.len() <= 8_192 => r.to_string(),
        _ => return drop(Refusal::Malformed, names),
    };
    let Some(approval) = event.approval.as_ref().and_then(Approval::parse) else {
        return drop(Refusal::NoApproval, names);
    };
    let Some(expires_ms) = event
        .expires_at
        .as_ref()
        .and_then(Value::as_str)
        .and_then(parse_iso_utc_ms)
    else {
        return drop(Refusal::Malformed, names);
    };
    if expires_ms - POST_MARGIN_MS <= now_ms {
        return drop(Refusal::Expired, names);
    }
    if malformed {
        return drop(Refusal::Malformed, names);
    }

    let refuse = |reason: Refusal| Decision::Refuse {
        request_id: request_id.clone(),
        receipt: receipt.clone(),
        approval,
        names: names.clone(),
        reason,
    };
    if paused {
        return refuse(Refusal::Paused);
    }
    if names.len() > MAX_CHECKS_PER_REQUEST {
        return refuse(Refusal::TooManyChecks);
    }
    let mut checks = Vec::with_capacity(items.len());
    for item in &items {
        let name = item
            .get("check")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(check) = CheckName::parse(name) else {
            return refuse(Refusal::UnknownCheck);
        };
        let Some(options) = parse_options(check, item.get("options")) else {
            return refuse(Refusal::BadOptions);
        };
        checks.push(CheckRequest { check, options });
    }

    Decision::Run {
        request_id,
        receipt,
        approval,
        checks,
        deadline_ms: (expires_ms - POST_MARGIN_MS).min(now_ms + RUN_BUDGET_MS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_790_579_662_546; // 2026-09-28T07:14:22.546Z

    fn event(approval: Value, checks: Value) -> RawCheckEvent {
        RawCheckEvent {
            request_id: Some(json!("0b6f1a52-4d7e-4a8e-9a39-2f0f0c7d5f11")),
            receipt: Some(json!("eyJ2IjoxfQ.sig")),
            approval: Some(approval),
            checks: Some(checks),
            expires_at: Some(json!("2026-09-28T07:14:52.546Z")),
        }
    }

    #[test]
    fn runs_an_approved_known_check() {
        let decision = decide(
            &event(json!("person"), json!([{ "check": "disk_space" }])),
            false,
            NOW,
        );
        match decision {
            Decision::Run {
                approval,
                checks,
                deadline_ms,
                ..
            } => {
                assert_eq!(approval, Approval::Person);
                assert_eq!(
                    checks,
                    vec![CheckRequest {
                        check: CheckName::DiskSpace,
                        options: CheckOptions::default()
                    }]
                );
                assert_eq!(deadline_ms, NOW + RUN_BUDGET_MS);
            }
            other => panic!("expected run, got {other:?}"),
        }
    }

    #[test]
    fn keeps_bounded_options() {
        let decision = decide(
            &event(
                json!("allow_all"),
                json!([{ "check": "recent_crashes", "options": { "limit": 5, "windowDays": 3 } }]),
            ),
            false,
            NOW,
        );
        let Decision::Run {
            approval, checks, ..
        } = decision
        else {
            panic!("expected run")
        };
        assert_eq!(approval, Approval::AllowAll);
        assert_eq!(
            checks[0].options,
            CheckOptions {
                limit: Some(5),
                window_days: Some(3)
            }
        );
    }

    #[test]
    fn refuses_everything_while_paused_including_allow_all() {
        for approval in ["person", "allow_all"] {
            let decision = decide(
                &event(
                    json!(approval),
                    json!([{ "check": "disk_space" }, { "check": "installed_apps" }]),
                ),
                true,
                NOW,
            );
            match decision {
                Decision::Refuse { reason, names, .. } => {
                    assert_eq!(reason, Refusal::Paused);
                    assert_eq!(names, vec!["disk_space", "installed_apps"]);
                }
                other => panic!("expected refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn refuses_unknown_check_names_and_runs_nothing() {
        let decision = decide(
            &event(
                json!("person"),
                json!([{ "check": "disk_space" }, { "check": "read_files" }]),
            ),
            false,
            NOW,
        );
        assert!(matches!(
            decision,
            Decision::Refuse {
                reason: Refusal::UnknownCheck,
                ..
            }
        ));
    }

    #[test]
    fn refuses_options_outside_the_bounds() {
        for options in [
            json!({ "limit": 0 }),
            json!({ "limit": 26 }),
            json!({ "limit": 2.5 }),
            json!({ "limit": "5" }),
            json!({ "path": 1 }),
            json!({ "windowDays": 3 }),
            json!(["limit"]),
        ] {
            let decision = decide(
                &event(
                    json!("person"),
                    json!([{ "check": "heavy_programs", "options": options }]),
                ),
                false,
                NOW,
            );
            assert!(
                matches!(
                    decision,
                    Decision::Refuse {
                        reason: Refusal::BadOptions,
                        ..
                    }
                ),
                "{options}"
            );
        }
        let no_options_allowed = decide(
            &event(
                json!("person"),
                json!([{ "check": "disk_space", "options": { "limit": 1 } }]),
            ),
            false,
            NOW,
        );
        assert!(matches!(
            no_options_allowed,
            Decision::Refuse {
                reason: Refusal::BadOptions,
                ..
            }
        ));
    }

    #[test]
    fn refuses_more_than_three_checks() {
        let decision = decide(
            &event(
                json!("person"),
                json!([{ "check": "disk_space" }, { "check": "network_status" }, { "check": "security_status" }, { "check": "installed_apps" }]),
            ),
            false,
            NOW,
        );
        assert!(matches!(
            decision,
            Decision::Refuse {
                reason: Refusal::TooManyChecks,
                ..
            }
        ));
    }

    #[test]
    fn drops_requests_without_approval() {
        for approval in [json!(null), json!("yes"), json!(true), json!("PERSON")] {
            let decision = decide(
                &event(approval.clone(), json!([{ "check": "disk_space" }])),
                false,
                NOW,
            );
            assert!(
                matches!(
                    decision,
                    Decision::Drop {
                        reason: Refusal::NoApproval,
                        ..
                    }
                ),
                "{approval}"
            );
        }
        let mut missing = event(json!("person"), json!([{ "check": "disk_space" }]));
        missing.approval = None;
        assert!(matches!(
            decide(&missing, false, NOW),
            Decision::Drop {
                reason: Refusal::NoApproval,
                ..
            }
        ));
    }

    #[test]
    fn drops_expired_and_malformed_requests() {
        let late = decide(
            &event(json!("person"), json!([{ "check": "disk_space" }])),
            false,
            NOW + 29_000,
        );
        assert!(matches!(
            late,
            Decision::Drop {
                reason: Refusal::Expired,
                ..
            }
        ));

        let mut bad_id = event(json!("person"), json!([{ "check": "disk_space" }]));
        bad_id.request_id = Some(json!("../../pair/claim"));
        assert!(matches!(
            decide(&bad_id, false, NOW),
            Decision::Drop {
                reason: Refusal::Malformed,
                ..
            }
        ));

        let mut no_receipt = event(json!("person"), json!([{ "check": "disk_space" }]));
        no_receipt.receipt = Some(json!(""));
        assert!(matches!(
            decide(&no_receipt, false, NOW),
            Decision::Drop {
                reason: Refusal::Malformed,
                ..
            }
        ));

        for checks in [
            json!([]),
            json!("disk_space"),
            json!([{ "check": "disk_space" }, { "check": "disk_space" }]),
            json!([{ "name": "disk_space" }]),
        ] {
            let decision = decide(&event(json!("person"), checks.clone()), false, NOW);
            assert!(
                matches!(
                    decision,
                    Decision::Drop {
                        reason: Refusal::Malformed,
                        ..
                    }
                ),
                "{checks}"
            );
        }
    }

    #[test]
    fn deadline_leaves_time_to_post_before_the_receipt_expires() {
        let mut soon = event(json!("person"), json!([{ "check": "disk_space" }]));
        soon.expires_at = Some(json!("2026-09-28T07:14:32.546Z"));
        let Decision::Run { deadline_ms, .. } = decide(&soon, false, NOW) else {
            panic!("expected run")
        };
        assert_eq!(deadline_ms, NOW + 10_000 - POST_MARGIN_MS);
    }
}
