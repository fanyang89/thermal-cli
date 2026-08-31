use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::{Channel, DeliveryError, Notification};

const CHANNEL_NAME: &str = "ServerChan";
const TURBO_API_ROOT: &str = "https://sctapi.ftqq.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ServerChan {
    client: Client,
    endpoint: String,
}

impl ServerChan {
    pub fn new(sendkey: impl AsRef<str>) -> Result<Self, DeliveryError> {
        let endpoint = endpoint_for_sendkey(sendkey.as_ref())?;
        Self::with_endpoint(endpoint)
    }

    /// Creates a channel for a ServerChan-compatible endpoint.
    ///
    /// This is useful for compatible gateways and integration tests. The URL
    /// is expected to be the complete `.send` endpoint.
    pub fn with_endpoint(endpoint: impl Into<String>) -> Result<Self, DeliveryError> {
        let endpoint = endpoint.into();
        reqwest::Url::parse(&endpoint).map_err(|_| delivery_error("invalid endpoint URL"))?;
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("push-notify/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| delivery_error("failed to initialize HTTP client"))?;
        Ok(Self { client, endpoint })
    }
}

impl Channel for ServerChan {
    fn name(&self) -> &str {
        CHANNEL_NAME
    }

    fn send(&self, notification: &Notification) -> Result<(), DeliveryError> {
        let title = serverchan_title(notification.title());
        let response = self
            .client
            .post(&self.endpoint)
            .form(&[("title", title.as_str()), ("desp", notification.body())])
            .send()
            .map_err(|_| delivery_error("request failed"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(http_error(status));
        }

        let response: ServerChanResponse = response
            .json()
            .map_err(|_| delivery_error("invalid API response"))?;
        if response.code != 0 {
            let message = response
                .message
                .as_deref()
                .map(safe_message)
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| format!("API returned code {}", response.code));
            return Err(delivery_error(message));
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ServerChanResponse {
    code: i64,
    #[serde(default, alias = "msg")]
    message: Option<String>,
}

fn endpoint_for_sendkey(sendkey: &str) -> Result<String, DeliveryError> {
    let sendkey = sendkey.trim();
    if sendkey.is_empty()
        || !sendkey
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(delivery_error("invalid SendKey"));
    }

    if let Some(serverchan3) = sendkey.strip_prefix("sctp") {
        let separator = serverchan3
            .find('t')
            .ok_or_else(|| delivery_error("invalid ServerChan³ SendKey"))?;
        let uid = &serverchan3[..separator];
        if uid.is_empty() || !uid.chars().all(|character| character.is_ascii_digit()) {
            return Err(delivery_error("invalid ServerChan³ SendKey"));
        }
        return Ok(format!("https://{uid}.push.ft07.com/send/{sendkey}.send"));
    }

    if !sendkey.starts_with("SCT") {
        return Err(delivery_error("SendKey must start with SCT or sctp"));
    }
    Ok(format!("{TURBO_API_ROOT}/{sendkey}.send"))
}

fn serverchan_title(title: &str) -> String {
    let normalized = title.replace(['\r', '\n'], " ");
    let normalized = normalized.trim();
    let normalized = if normalized.is_empty() {
        "Notification"
    } else {
        normalized
    };
    normalized.chars().take(32).collect()
}

fn safe_message(message: &str) -> String {
    message
        .replace(['\r', '\n'], " ")
        .chars()
        .take(160)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn delivery_error(message: impl Into<String>) -> DeliveryError {
    DeliveryError::new(CHANNEL_NAME, message)
}

fn http_error(status: StatusCode) -> DeliveryError {
    delivery_error(format!("HTTP status {}", status.as_u16()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn builds_turbo_endpoint() {
        assert_eq!(
            endpoint_for_sendkey("SCT123abc").unwrap(),
            "https://sctapi.ftqq.com/SCT123abc.send"
        );
    }

    #[test]
    fn builds_serverchan3_endpoint() {
        assert_eq!(
            endpoint_for_sendkey("sctp12345tSecret").unwrap(),
            "https://12345.push.ft07.com/send/sctp12345tSecret.send"
        );
    }

    #[test]
    fn rejects_malformed_sendkeys() {
        assert!(endpoint_for_sendkey("").is_err());
        assert!(endpoint_for_sendkey("token/with/slashes").is_err());
        assert!(endpoint_for_sendkey("sctpNoSeparator").is_err());
        assert!(endpoint_for_sendkey("OTHER123").is_err());
    }

    #[test]
    fn normalizes_serverchan_titles() {
        assert_eq!(serverchan_title("CPU\noverheat"), "CPU overheat");
        assert_eq!(serverchan_title("  \n  "), "Notification");
        assert_eq!(serverchan_title(&"x".repeat(40)).chars().count(), 32);
    }

    #[test]
    fn sends_form_and_accepts_success_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            request_tx.send(request).unwrap();
            let body = r#"{"code":0,"message":"success"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let channel = ServerChan::with_endpoint(format!("http://{address}/send")).unwrap();

        channel
            .send(&Notification::new("Thermal alert", "CPU is hot"))
            .unwrap();

        let request = request_rx.recv().unwrap();
        assert!(request.starts_with("POST /send HTTP/1.1"));
        assert!(request.contains("title=Thermal+alert"));
        assert!(request.contains("desp=CPU+is+hot"));
        server.join().unwrap();
    }

    fn read_http_request(stream: &mut impl Read) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let mut expected_length = None;
        loop {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..count]);
            if expected_length.is_none()
                && let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let body_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                expected_length = Some(header_end + 4 + body_length);
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        String::from_utf8(request).unwrap()
    }
}
