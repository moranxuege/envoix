//! HTTP adapter for the opaque receipt-mailbox lane.

#![forbid(unsafe_code)]

use std::fmt;

use envoix_protocol::mailbox::identifiers::RECEIPT_HTTP_ROUTE;
use envoix_protocol::mailbox::{ReceiptSlot, SealedReceipt};
use reqwest::{StatusCode, Url};

#[derive(Clone)]
pub struct HttpReceiptMailbox {
    endpoint: Url,
    client: reqwest::Client,
}

impl HttpReceiptMailbox {
    pub fn new(endpoint: &str) -> Result<Self, MailboxClientError> {
        let mut endpoint = Url::parse(endpoint).map_err(|_| MailboxClientError::InvalidEndpoint)?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.cannot_be_a_base()
            || endpoint.host().is_none()
        {
            return Err(MailboxClientError::InvalidEndpoint);
        }
        endpoint.set_path("/");
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            endpoint,
            client: reqwest::Client::new(),
        })
    }

    pub async fn post(
        &self,
        slot: ReceiptSlot,
        sealed: &SealedReceipt,
    ) -> Result<(), MailboxClientError> {
        let response = self
            .client
            .post(self.receipt_url(slot)?)
            .body(sealed.as_bytes().to_vec())
            .send()
            .await
            .map_err(|_| MailboxClientError::Transport)?;
        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::CREATED => Ok(()),
            status => Err(MailboxClientError::UnexpectedStatus(status.as_u16())),
        }
    }

    pub async fn poll(
        &self,
        slot: ReceiptSlot,
    ) -> Result<Option<SealedReceipt>, MailboxClientError> {
        let response = self
            .client
            .get(self.receipt_url(slot)?)
            .send()
            .await
            .map_err(|_| MailboxClientError::Transport)?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::OK => {
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|_| MailboxClientError::Transport)?;
                let sealed = SealedReceipt::from_bytes(bytes.to_vec())
                    .map_err(|_| MailboxClientError::InvalidBlob)?;
                Ok(Some(sealed))
            }
            status => Err(MailboxClientError::UnexpectedStatus(status.as_u16())),
        }
    }

    fn receipt_url(&self, slot: ReceiptSlot) -> Result<Url, MailboxClientError> {
        let path = RECEIPT_HTTP_ROUTE.replace("{slot}", &slot.path_component());
        self.endpoint
            .join(&path)
            .map_err(|_| MailboxClientError::InvalidEndpoint)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailboxClientError {
    InvalidEndpoint,
    Transport,
    UnexpectedStatus(u16),
    InvalidBlob,
}

impl fmt::Display for MailboxClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint => formatter.write_str("invalid mailbox endpoint"),
            Self::Transport => formatter.write_str("mailbox transport failed"),
            Self::UnexpectedStatus(status) => {
                write!(formatter, "mailbox returned HTTP status {status}")
            }
            Self::InvalidBlob => formatter.write_str("mailbox returned an invalid sealed blob"),
        }
    }
}

impl std::error::Error for MailboxClientError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_must_be_http_base() {
        assert!(HttpReceiptMailbox::new("http://127.0.0.1:9460").is_ok());
        assert!(matches!(
            HttpReceiptMailbox::new("not a url"),
            Err(MailboxClientError::InvalidEndpoint)
        ));
        assert!(matches!(
            HttpReceiptMailbox::new("file:///tmp/mailbox"),
            Err(MailboxClientError::InvalidEndpoint)
        ));
    }
}
