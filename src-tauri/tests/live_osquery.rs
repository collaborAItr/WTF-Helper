//! Runs every fixed query, and every check, against the real bundled osquery on
//! this computer. Ignored by default because it needs the osquery binary; CI runs
//! it on macOS and Windows with `cargo test -- --ignored`.

use std::path::PathBuf;

use wtf_helper_lib::checks::osquery::{Osquery, RealMachine};
use wtf_helper_lib::checks::queries::{queries_for, Platform};
use wtf_helper_lib::checks::redact::Redactor;
use wtf_helper_lib::checks::run_batch;
use wtf_helper_lib::protocol::{CheckName, CheckOptions, CheckRequest, DATA_MAX_CHARS};
use wtf_helper_lib::relay::Api;
use wtf_helper_lib::timeutil::now_ms;

fn osquery() -> Osquery {
    let binary = std::env::var_os("OSQUERY_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let name = if cfg!(windows) {
                "osquery-x86_64-pc-windows-msvc.exe"
            } else {
                "osquery-universal-apple-darwin"
            };
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join(name)
        });
    let home = std::env::temp_dir().join("wtf-helper-live-osquery-home");
    Osquery::at(binary, home).expect("run `npm run fetch-osquery` first")
}

#[tokio::test]
#[ignore]
async fn every_fixed_query_runs_as_this_user() {
    let osquery = osquery();
    let platform = Platform::current();
    let mut failures = Vec::new();
    for check in CheckName::ALL {
        for query in queries_for(check, platform) {
            match osquery.run(&query).await {
                Ok(rows) => println!(
                    "{:>5} rows  {:<16} {}",
                    rows.len(),
                    check.as_str(),
                    query.sql()
                ),
                Err(error) => failures.push(format!("{}: {error}", query.sql())),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "queries that failed:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
#[ignore]
async fn every_check_answers_within_the_limits() {
    let machine = RealMachine {
        osquery: Ok(osquery()),
        redactor: Redactor::for_this_computer(),
        api: Api::official(),
    };
    #[allow(deprecated)]
    let home = std::env::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    for check in CheckName::ALL {
        let request = CheckRequest {
            check,
            options: CheckOptions::default(),
        };
        let started = now_ms();
        let result = run_batch(&machine, &[request], now_ms() + 20_000)
            .await
            .remove(0);
        let json = serde_json::to_string(&result).unwrap();
        println!(
            "{} ({} ms): {}",
            check.as_str(),
            now_ms() - started,
            result
                .summary
                .as_deref()
                .or(result.error.as_deref())
                .unwrap_or("")
        );
        assert!(result.ok, "{} failed: {:?}", check.as_str(), result.error);
        let data_len = serde_json::to_string(result.data.as_ref().unwrap())
            .unwrap()
            .chars()
            .count();
        assert!(
            data_len <= DATA_MAX_CHARS,
            "{} data is {data_len} characters",
            check.as_str()
        );
        assert!(result.summary.as_ref().unwrap().chars().count() <= 500);
        if home.len() > 3 {
            assert!(
                !json.contains(&home),
                "{} leaked the home folder",
                check.as_str()
            );
        }
    }
}
