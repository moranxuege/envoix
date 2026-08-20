//! Client for the owner-local Envoix Agent control transport.

use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::product::{
    AgentRequest, AgentRequestEnvelope, AgentResponse, AgentResponseEnvelope,
    MAX_AGENT_REQUEST_BYTES, MAX_AGENT_RESPONSE_BYTES, default_agent_control_endpoint,
};

/// A controller for the Agent endpoint owned by the current desktop user.
#[derive(Clone, Debug)]
pub struct AgentControlClient {
    endpoint: PathBuf,
}

impl AgentControlClient {
    pub fn new(endpoint: impl Into<PathBuf>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    pub fn for_current_user() -> io::Result<Self> {
        Ok(Self::new(default_agent_control_endpoint()?))
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    #[cfg(unix)]
    pub async fn call(&self, request: AgentRequest) -> io::Result<AgentResponse> {
        let stream = tokio::net::UnixStream::connect(&self.endpoint)
            .await
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "cannot connect to Envoix Agent at {}: {error}; run `envoix agent start` or start envoix-agent in a foreground shell",
                        self.endpoint.display()
                    ),
                )
            })?;
        call_agent_stream(stream, request).await
    }

    #[cfg(windows)]
    pub async fn call(&self, request: AgentRequest) -> io::Result<AgentResponse> {
        use std::time::Duration;

        use tokio::net::windows::named_pipe::ClientOptions;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let stream = loop {
            match ClientOptions::new().open(&self.endpoint) {
                Ok(stream) => break stream,
                Err(error)
                    if error.raw_os_error()
                        == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32)
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!(
                            "cannot connect to Envoix Agent at {}: {error}; start envoix-agent for this user",
                            self.endpoint.display()
                        ),
                    ));
                }
            }
        };
        call_agent_stream(stream, request).await
    }

    #[cfg(not(any(unix, windows)))]
    pub async fn call(&self, _request: AgentRequest) -> io::Result<AgentResponse> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the local Envoix Agent control transport is unsupported on this platform",
        ))
    }
}

async fn call_agent_stream<S>(stream: S, request: AgentRequest) -> io::Result<AgentResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let request_id = agent_request_id()?;
    let envelope = AgentRequestEnvelope::new(request_id.clone(), request)?;
    let mut bytes = serde_json::to_vec(&envelope).map_err(invalid_control_data)?;
    if bytes.len() as u64 > MAX_AGENT_REQUEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Agent request exceeds the control message limit",
        ));
    }
    bytes.push(b'\n');
    write.write_all(&bytes).await?;
    write.shutdown().await?;

    let mut response_bytes = Vec::new();
    let mut limited = BufReader::new(read).take(MAX_AGENT_RESPONSE_BYTES + 1);
    limited.read_until(b'\n', &mut response_bytes).await?;
    if response_bytes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Envoix Agent closed the control connection without a response",
        ));
    }
    if response_bytes.len() as u64 > MAX_AGENT_RESPONSE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent response exceeds the control message limit",
        ));
    }
    let response: AgentResponseEnvelope =
        serde_json::from_slice(&response_bytes).map_err(invalid_control_data)?;
    response.validate_for(&request_id)?;
    Ok(response.response)
}

fn agent_request_id() -> io::Result<String> {
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random)
        .map_err(|error| io::Error::other(format!("request ID entropy unavailable: {error}")))?;
    Ok(format!("cli_{}", URL_SAFE_NO_PAD.encode(random)))
}

fn invalid_control_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn respond(stream: tokio::io::DuplexStream, request_id: impl FnOnce(&str) -> String) {
        let (read, mut write) = tokio::io::split(stream);
        let mut line = String::new();
        BufReader::new(read).read_line(&mut line).await.unwrap();
        let request: AgentRequestEnvelope = serde_json::from_str(&line).unwrap();
        request.validate().unwrap();
        let response = AgentResponseEnvelope::new(
            request_id(&request.request_id),
            AgentResponse::error("test", "response"),
        )
        .unwrap();
        let mut bytes = serde_json::to_vec(&response).unwrap();
        bytes.push(b'\n');
        write.write_all(&bytes).await.unwrap();
    }

    #[tokio::test]
    async fn stream_round_trips_a_valid_envelope() {
        let (client, server) = tokio::io::duplex(4_096);
        let server = tokio::spawn(respond(server, str::to_owned));

        let response = call_agent_stream(client, AgentRequest::Status)
            .await
            .unwrap();

        server.await.unwrap();
        assert_eq!(response, AgentResponse::error("test", "response"));
    }

    #[tokio::test]
    async fn stream_rejects_a_response_for_another_request() {
        let (client, server) = tokio::io::duplex(4_096);
        let server = tokio::spawn(respond(server, |_| "request_other".to_string()));

        let error = call_agent_stream(client, AgentRequest::Status)
            .await
            .unwrap_err();

        server.await.unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "Agent response request ID does not match the command"
        );
    }

    #[tokio::test]
    async fn stream_rejects_an_empty_response() {
        let (client, server) = tokio::io::duplex(4_096);
        let server = tokio::spawn(async move {
            let mut line = String::new();
            BufReader::new(server).read_line(&mut line).await.unwrap();
        });

        let error = call_agent_stream(client, AgentRequest::Status)
            .await
            .unwrap_err();

        server.await.unwrap();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(
            error.to_string(),
            "Envoix Agent closed the control connection without a response"
        );
    }

    #[tokio::test]
    async fn stream_rejects_an_oversized_request_before_writing() {
        let (client, _server) = tokio::io::duplex(4_096);
        let error = call_agent_stream(
            client,
            AgentRequest::Pair {
                label: "x".repeat(MAX_AGENT_REQUEST_BYTES as usize),
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            error.to_string(),
            "Agent request exceeds the control message limit"
        );
    }
}
