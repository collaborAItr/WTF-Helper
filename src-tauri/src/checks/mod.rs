//! The eight v1 checks. Each runs fixed osquery queries, keeps only the fields
//! listed in CHECKS.md, redacts the home folder and username, and caps the size.

pub mod lists;
pub mod osquery;
pub mod queries;
pub mod redact;
pub mod summaries;

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use futures_util::future::join_all;
use serde_json::{json, Value};

use crate::protocol::{
    clip_chars, CheckName, CheckRequest, CheckResult, DATA_MAX_CHARS, SUMMARY_MAX_CHARS,
};
use queries::{FixedQuery, Platform};
use redact::Redactor;

/// One osquery row. `--json` output gives every column as a string.
pub type Row = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    pub reachable: bool,
    pub ms: Option<u64>,
    pub status: Option<u16>,
}

/// What a check may touch: fixed queries, a reachability probe to our own host, and a clock.
pub trait Machine: Sync {
    fn platform(&self) -> Platform;
    fn redactor(&self) -> &Redactor;
    fn service_host(&self) -> &str;
    fn query(&self, query: &FixedQuery) -> impl Future<Output = Result<Vec<Row>, String>> + Send;
    fn reach_service(&self) -> impl Future<Output = Reach> + Send;
    fn pause(&self, ms: u64) -> impl Future<Output = ()> + Send;
    fn now_ms(&self) -> i64;
}

pub struct Outcome {
    pub summary: String,
    pub data: Value,
}

pub fn field<'a>(row: &'a Row, key: &str) -> &'a str {
    row.get(key).map(|v| v.trim()).unwrap_or("")
}

pub fn field_u64(row: &Row, key: &str) -> Option<u64> {
    field(row, key).parse().ok()
}

pub fn field_i64(row: &Row, key: &str) -> Option<i64> {
    field(row, key).parse().ok()
}

pub fn gb(bytes: u64) -> f64 {
    (bytes as f64 / 1_073_741_824.0 * 10.0).round() / 10.0
}

/// Plain error text for a failed check. Raw osquery errors can carry paths, so they are not sent.
pub const QUERY_FAILED: &str = "The computer did not answer this check.";
pub const TOO_SLOW: &str = "This check took too long.";

async fn run_one<M: Machine>(machine: &M, request: &CheckRequest) -> Result<Outcome, String> {
    match request.check {
        CheckName::SystemOverview => summaries::system_overview(machine).await,
        CheckName::DiskSpace => summaries::disk_space(machine).await,
        CheckName::NetworkStatus => summaries::network_status(machine).await,
        CheckName::SecurityStatus => summaries::security_status(machine).await,
        CheckName::HeavyPrograms => lists::heavy_programs(machine, &request.options).await,
        CheckName::StartupItems => lists::startup_items(machine, &request.options).await,
        CheckName::RecentCrashes => lists::recent_crashes(machine, &request.options).await,
        CheckName::InstalledApps => lists::installed_apps(machine, &request.options).await,
    }
}

/// Redacts, then keeps the serialized data under `DATA_MAX_CHARS` by dropping list rows from the end.
pub fn finish(check: CheckName, outcome: Outcome, redactor: &Redactor) -> CheckResult {
    let mut data = outcome.data;
    redactor.value(&mut data);
    let summary = clip_chars(&redactor.text(&outcome.summary), SUMMARY_MAX_CHARS);
    fit_data(&mut data);
    CheckResult::success(check, summary, data)
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_string(value)
        .map(|s| s.chars().count())
        .unwrap_or(usize::MAX)
}

pub fn fit_data(data: &mut Value) {
    if serialized_len(data) <= DATA_MAX_CHARS {
        return;
    }
    if let Some(items) = data.get_mut("items").and_then(Value::as_array_mut) {
        let mut low = 0;
        let mut high = items.len();
        let all = items.clone();
        // Largest prefix of rows that fits.
        while low < high {
            let mid = (low + high).div_ceil(2);
            items.clear();
            items.extend_from_slice(&all[..mid]);
            if serialized_len(&Value::Array(items.clone())) + 64 <= DATA_MAX_CHARS {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        items.clear();
        items.extend_from_slice(&all[..low]);
        data["truncated"] = json!(true);
    }
    if serialized_len(data) > DATA_MAX_CHARS {
        *data = json!({ "truncated": true });
    }
}

/// Runs the checks side by side and returns one result per request, in order.
pub async fn run_batch<M: Machine>(
    machine: &M,
    checks: &[CheckRequest],
    deadline_ms: i64,
) -> Vec<CheckResult> {
    let budget = Duration::from_millis((deadline_ms - machine.now_ms()).max(0) as u64);
    let runs = checks.iter().map(|request| async move {
        match tokio::time::timeout(budget, run_one(machine, request)).await {
            Ok(Ok(outcome)) => finish(request.check, outcome, machine.redactor()),
            Ok(Err(error)) => CheckResult::failure(request.check.as_str(), &error),
            Err(_) => CheckResult::failure(request.check.as_str(), TOO_SLOW),
        }
    });
    join_all(runs).await
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Answers fixed queries from canned rows. Unknown queries fail, like a missing table.
    pub struct FakeMachine {
        pub platform: Platform,
        pub redactor: Redactor,
        pub answers: HashMap<String, Vec<Vec<Row>>>,
        pub asked: Mutex<Vec<String>>,
        pub reach: Reach,
        pub slow_ms: u64,
        pub now: i64,
    }

    pub fn row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    impl FakeMachine {
        pub fn new(platform: Platform) -> Self {
            FakeMachine {
                platform,
                redactor: Redactor::new(Some("/Users/pat"), Some("pat"), false),
                answers: HashMap::new(),
                asked: Mutex::new(Vec::new()),
                reach: Reach {
                    reachable: true,
                    ms: Some(42),
                    status: Some(200),
                },
                slow_ms: 0,
                now: 1_790_579_662_546,
            }
        }

        /// Each call to the same query takes the next answer; the last one repeats.
        pub fn answer(mut self, query: &FixedQuery, rows: Vec<Row>) -> Self {
            self.answers
                .entry(query.sql().to_string())
                .or_default()
                .push(rows);
            self
        }
    }

    impl Machine for FakeMachine {
        fn platform(&self) -> Platform {
            self.platform
        }
        fn redactor(&self) -> &Redactor {
            &self.redactor
        }
        fn service_host(&self) -> &str {
            "api.collaboraitr.com"
        }
        async fn query(&self, query: &FixedQuery) -> Result<Vec<Row>, String> {
            if self.slow_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.slow_ms)).await;
            }
            let mut asked = self.asked.lock().unwrap();
            let sql = query.sql().to_string();
            let seen = asked.iter().filter(|q| **q == sql).count();
            asked.push(sql.clone());
            match self.answers.get(&sql) {
                Some(answers) => Ok(answers[seen.min(answers.len() - 1)].clone()),
                None => Err(format!("no such table in: {sql}")),
            }
        }
        async fn reach_service(&self) -> Reach {
            self.reach.clone()
        }
        async fn pause(&self, _ms: u64) {}
        fn now_ms(&self) -> i64 {
            self.now
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use crate::protocol::CheckOptions;

    #[test]
    fn fit_keeps_list_data_under_the_cap() {
        let items: Vec<Value> = (0..2_000).map(|i| json!({ "name": format!("App number {i}"), "label": format!("App number {i}") })).collect();
        let mut data = json!({ "items": items, "total": 2_000, "truncated": false });
        fit_data(&mut data);
        let len = serde_json::to_string(&data).unwrap().chars().count();
        assert!(len <= DATA_MAX_CHARS, "{len}");
        assert_eq!(data["truncated"], json!(true));
        assert_eq!(data["total"], json!(2_000));
        assert!(data["items"].as_array().unwrap().len() > 50);
    }

    #[tokio::test]
    async fn batch_returns_one_result_per_check_in_order() {
        let machine = FakeMachine::new(Platform::Mac).answer(
            &queries::MAC_MOUNTS,
            vec![row(&[
                ("path", "/"),
                ("device", "/dev/disk1s1s1"),
                ("type", "apfs"),
                ("blocks_size", "4096"),
                ("blocks", "1000000"),
                ("blocks_available", "250000"),
            ])],
        );
        let checks = vec![
            CheckRequest {
                check: CheckName::DiskSpace,
                options: CheckOptions::default(),
            },
            CheckRequest {
                check: CheckName::InstalledApps,
                options: CheckOptions::default(),
            },
        ];
        let results = run_batch(&machine, &checks, machine.now + 5_000).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].check, "disk_space");
        assert!(results[0].ok);
        assert_eq!(results[1].check, "installed_apps");
        assert!(!results[1].ok);
        assert_eq!(results[1].error.as_deref(), Some(QUERY_FAILED));
    }

    #[tokio::test(start_paused = true)]
    async fn slow_checks_fail_cleanly_at_the_deadline() {
        let mut machine = FakeMachine::new(Platform::Mac).answer(&queries::MAC_MOUNTS, vec![]);
        machine.slow_ms = 60_000;
        let checks = vec![CheckRequest {
            check: CheckName::DiskSpace,
            options: CheckOptions::default(),
        }];
        let results = run_batch(&machine, &checks, machine.now + 1_000).await;
        assert_eq!(results[0].error.as_deref(), Some(TOO_SLOW));
    }
}
