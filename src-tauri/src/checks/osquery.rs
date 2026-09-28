//! Runs the bundled osquery in shell mode, as the signed-in user, one fixed query at a time.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::queries::{FixedQuery, Platform};
use super::redact::Redactor;
use super::{Machine, Reach, Row};
use crate::relay::Api;
use crate::timeutil::now_ms;

const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct Osquery {
    binary: PathBuf,
    home: PathBuf,
}

impl Osquery {
    /// The bundled binary sits next to the app's own executable.
    pub fn locate(scratch_home: PathBuf) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = exe.parent().ok_or("no executable folder")?;
        let name = if cfg!(windows) {
            "osquery.exe"
        } else {
            "osquery"
        };
        Self::at(dir.join(name), scratch_home)
    }

    pub fn at(binary: PathBuf, scratch_home: PathBuf) -> Result<Self, String> {
        if !binary.is_file() {
            return Err(format!("osquery is missing at {}", binary.display()));
        }
        Ok(Osquery {
            binary,
            home: scratch_home,
        })
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub async fn run(&self, query: &FixedQuery) -> Result<Vec<Row>, String> {
        std::fs::create_dir_all(&self.home).map_err(|e| e.to_string())?;
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .args([
                "-S",
                "--json",
                "--disable_extensions=true",
                "--disable_database=true",
                "--disable_events=true",
                "--logger_min_stderr=3",
            ])
            .arg(query.sql())
            // osquery's shell keeps a history folder in $HOME; point it at our cache instead.
            // Tables that read each user's files find them through the user database, not $HOME.
            .env("HOME", &self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let output = tokio::time::timeout(QUERY_TIMEOUT, command.output())
            .await
            .map_err(|_| "osquery timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!("osquery exited with {}", output.status));
        }
        parse_rows(&output.stdout)
    }
}

pub fn parse_rows(stdout: &[u8]) -> Result<Vec<Row>, String> {
    let value: Value = serde_json::from_slice(stdout).map_err(|e| e.to_string())?;
    let list = value.as_array().ok_or("osquery did not return a list")?;
    Ok(list
        .iter()
        .filter_map(Value::as_object)
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| {
                    let text = match v {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    (k.clone(), text)
                })
                .collect()
        })
        .collect())
}

pub struct RealMachine {
    pub osquery: Result<Osquery, String>,
    pub redactor: Redactor,
    pub api: Api,
}

impl Machine for RealMachine {
    fn platform(&self) -> Platform {
        Platform::current()
    }

    fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    fn service_host(&self) -> &str {
        crate::flavor::FLAVOR.host
    }

    async fn query(&self, query: &FixedQuery) -> Result<Vec<Row>, String> {
        match &self.osquery {
            Ok(osquery) => osquery.run(query).await,
            Err(error) => Err(error.clone()),
        }
    }

    async fn reach_service(&self) -> Reach {
        let started = Instant::now();
        match self.api.health().await {
            Some(status) => Reach {
                reachable: true,
                ms: Some(started.elapsed().as_millis() as u64),
                status: Some(status),
            },
            None => Reach {
                reachable: false,
                ms: None,
                status: None,
            },
        }
    }

    async fn pause(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    fn now_ms(&self) -> i64 {
        now_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_osquery_json_rows() {
        let rows = parse_rows(br#"[{"name":"macOS","version":"12.7.3","n":3,"x":null}]"#).unwrap();
        assert_eq!(rows[0]["name"], "macOS");
        assert_eq!(rows[0]["n"], "3");
        assert_eq!(rows[0]["x"], "");
        assert!(parse_rows(b"Error: no such table").is_err());
        assert!(parse_rows(b"[\n\n]").unwrap().is_empty());
    }
}
