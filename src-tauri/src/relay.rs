//! Outbound calls to the pinned service: claim a pairing code, hold the check
//! stream, post results. The helper never listens on a port.

use std::time::Duration;

use reqwest::{redirect, Client, Response, StatusCode, Url};
use serde::de::DeserializeOwned;

use crate::flavor::{FLAVOR, HELPER_VERSION};
use crate::protocol::{ClaimData, ClaimRequest, Envelope, ResultBody};

pub const TOKEN_HEADER: &str = "X-WTF-Helper-Token";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 401: missing, unknown or revoked token. The pairing is over.
    Unauthorized { code: String },
    /// 404 `not_enabled`: the service has the helper switched off.
    NotEnabled,
    /// Any other answer from the service.
    Rejected {
        status: u16,
        code: String,
        message: String,
        recoverable: bool,
    },
    /// No answer: offline, DNS, TLS, timeout.
    Network(String),
}

impl ApiError {
    /// Plain words for the helper window.
    pub fn plain(&self) -> String {
        match self {
            ApiError::Unauthorized { .. } => {
                "This computer is no longer connected to your account.".into()
            }
            ApiError::NotEnabled => "WTF Helper is not switched on for your account yet.".into(),
            ApiError::Rejected { code, message, .. } => match code.as_str() {
                "rate_limited" => "Too many tries. Wait a few minutes and try again.".into(),
                _ if !message.is_empty() => message.clone(),
                _ => "The service could not do that. Try again.".into(),
            },
            ApiError::Network(_) => format!(
                "Could not reach {}. Check the internet connection and try again.",
                FLAVOR.host
            ),
        }
    }
}

#[derive(Clone)]
pub struct Api {
    base: String,
    origin: String,
    client: Client,
    stream_client: Client,
}

fn build_client(https_only: bool, total_timeout: Option<Duration>) -> Client {
    let mut builder = Client::builder()
        .redirect(redirect::Policy::none())
        .https_only(https_only)
        .connect_timeout(Duration::from_secs(10))
        .user_agent(format!("WTF-Helper/{HELPER_VERSION}"));
    if let Some(t) = total_timeout {
        builder = builder.timeout(t);
    }
    builder.build().expect("HTTP client")
}

impl Api {
    /// The only constructor outside tests. Always this build's pinned host.
    pub fn official() -> Self {
        Self::with_base(FLAVOR.api_base(), FLAVOR.origin())
    }

    fn with_base(base: String, origin: String) -> Self {
        let https_only = base.starts_with("https://");
        Api {
            client: build_client(https_only, Some(Duration::from_secs(15))),
            stream_client: build_client(https_only, None),
            base,
            origin,
        }
    }

    #[cfg(test)]
    pub fn for_tests(origin: &str) -> Self {
        Self::with_base(format!("{origin}/api/wtf/helper"), origin.to_string())
    }

    fn url(&self, path: &str) -> Result<Url, ApiError> {
        let url = Url::parse(&format!("{}{}", self.base, path))
            .map_err(|e| ApiError::Network(e.to_string()))?;
        #[cfg(not(test))]
        if !FLAVOR.is_pinned(&url) {
            return Err(ApiError::Network(
                "refused an address outside the pinned host".into(),
            ));
        }
        Ok(url)
    }

    async fn read_error(response: Response) -> ApiError {
        let status = response.status();
        let body: Option<Envelope<serde_json::Value>> = response.json().await.ok();
        let code = body
            .as_ref()
            .and_then(|b| b.code.clone())
            .unwrap_or_default();
        if status == StatusCode::UNAUTHORIZED {
            return ApiError::Unauthorized { code };
        }
        if status == StatusCode::NOT_FOUND && code == "not_enabled" {
            return ApiError::NotEnabled;
        }
        let message = body
            .as_ref()
            .and_then(|b| b.message.clone().or_else(|| b.error.clone()))
            .unwrap_or_default();
        ApiError::Rejected {
            status: status.as_u16(),
            recoverable: body.as_ref().map(|b| b.recoverable).unwrap_or(false),
            code,
            message,
        }
    }

    async fn read_data<T: DeserializeOwned>(response: Response) -> Result<T, ApiError> {
        if !response.status().is_success() {
            return Err(Self::read_error(response).await);
        }
        let envelope: Envelope<T> = response
            .json()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        envelope
            .data
            .filter(|_| envelope.success)
            .ok_or(ApiError::Rejected {
                status: 200,
                code: "invalid_response".into(),
                message: String::new(),
                recoverable: false,
            })
    }

    pub async fn claim(&self, request: &ClaimRequest) -> Result<ClaimData, ApiError> {
        let response = self
            .client
            .post(self.url("/pair/claim")?)
            .json(request)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Self::read_data(response).await
    }

    pub async fn open_stream(&self, token: &str) -> Result<Response, ApiError> {
        let response = self
            .stream_client
            .get(self.url("/stream")?)
            .header(TOKEN_HEADER, token)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(Self::read_error(response).await);
        }
        Ok(response)
    }

    pub async fn post_result(
        &self,
        token: &str,
        request_id: &str,
        body: &ResultBody<'_>,
    ) -> Result<(), ApiError> {
        let response = self
            .client
            .post(self.url(&format!("/checks/{request_id}/result"))?)
            .header(TOKEN_HEADER, token)
            .json(body)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Self::read_error(response).await)
        }
    }

    /// Status of `GET {origin}/health`, or None if nothing answered within 5 seconds.
    pub async fn health(&self) -> Option<u16> {
        let url = Url::parse(&format!("{}/health", self.origin)).ok()?;
        #[cfg(not(test))]
        if !FLAVOR.is_pinned(&url) {
            return None;
        }
        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()?;
        Some(response.status().as_u16())
    }
}

/// Splits a server-sent event byte stream into `data` payloads.
#[derive(Default)]
pub struct SseParser {
    buffer: String,
    data: Vec<String>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(end) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=end).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_string());
            }
            // Comments (":…") and other fields are ignored.
        }
        // A single event can never be this large; drop a runaway buffer.
        if self.buffer.len() > 1_000_000 {
            self.buffer.clear();
            self.data.clear();
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_handles_split_chunks_and_crlf() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"data: {\"type\":\"heart").is_empty());
        assert_eq!(
            parser.push(b"beat\"}\r\n\r\n: comment\n\ndata: [DONE]\n\n"),
            vec![r#"{"type":"heartbeat"}"#, "[DONE]"]
        );
        assert_eq!(parser.push(b"data:a\ndata: b\n\n"), vec!["a\nb"]);
    }

    #[test]
    fn plain_errors_do_not_leak_raw_details() {
        assert_eq!(
            ApiError::Network("dns error: failed to lookup".into()).plain(),
            format!(
                "Could not reach {}. Check the internet connection and try again.",
                FLAVOR.host
            )
        );
        assert_eq!(
            ApiError::Rejected {
                status: 400,
                code: "code_expired".into(),
                message: "That code has expired. Ask for a new one.".into(),
                recoverable: false
            }
            .plain(),
            "That code has expired. Ask for a new one."
        );
    }
}
