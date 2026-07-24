//! The message view-model shared between the backend (which builds it from
//! AMQP messages) and the UI (which renders it). Plain data, no AMQP types.

use std::time::Duration;

use time::OffsetDateTime;

use crate::body::DecodedBody;

/// Message state as reported by peek (`Active`, `Deferred`, `Scheduled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageState {
    #[default]
    Active,
    Deferred,
    Scheduled,
}

impl MessageState {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deferred => "deferred",
            Self::Scheduled => "scheduled",
        }
    }
}

/// A peeked or received message, decoded for display.
#[derive(Debug, Clone)]
pub struct SiftMessage {
    pub sequence_number: i64,
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub content_type: Option<String>,
    pub correlation_id: Option<String>,
    pub session_id: Option<String>,
    pub reply_to: Option<String>,
    pub to: Option<String>,
    pub enqueued_time: Option<OffsetDateTime>,
    pub expires_at: Option<OffsetDateTime>,
    pub time_to_live: Option<Duration>,
    pub delivery_count: Option<u32>,
    pub state: MessageState,
    /// Present only for messages received in peek-lock mode; keys the
    /// backend's lock registry for settle operations.
    pub lock_token: Option<String>,
    pub locked_until: Option<OffsetDateTime>,
    pub dead_letter_reason: Option<String>,
    pub dead_letter_error_description: Option<String>,
    pub dead_letter_source: Option<String>,
    /// Custom properties rendered as strings for display.
    pub application_properties: Vec<(String, String)>,
    pub body: DecodedBody,
}

impl SiftMessage {
    /// Prefill an outbound message from this one (resend/resubmit): keeps the
    /// body and user-settable properties, drops all system properties —
    /// including dead-letter metadata, so a resubmitted message is a clean
    /// copy rather than one that still reads as dead-lettered.
    #[must_use]
    pub fn to_outbound(&self) -> OutboundMessage {
        OutboundMessage {
            body: self.body.text.clone().unwrap_or_default(),
            raw_bytes: self.body.text.is_none().then(|| self.body.bytes.clone()),
            message_id: None, // a resent message gets a fresh id
            subject: self.subject.clone(),
            content_type: self.content_type.clone(),
            correlation_id: self.correlation_id.clone(),
            session_id: self.session_id.clone(),
            to: self.to.clone(),
            reply_to: self.reply_to.clone(),
            time_to_live: self.time_to_live,
            application_properties: self
                .application_properties
                .iter()
                .filter(|(k, _)| !is_dead_letter_property(k))
                .cloned()
                .collect(),
        }
    }
}

/// Dead-letter metadata the broker stores as application properties; these
/// must not be carried onto a resubmitted message.
fn is_dead_letter_property(key: &str) -> bool {
    matches!(
        key,
        "DeadLetterReason" | "DeadLetterErrorDescription" | "DeadLetterSource"
    )
}

/// A message to send, composed in the UI.
#[derive(Debug, Clone, Default)]
pub struct OutboundMessage {
    /// Text body; used unless `raw_bytes` is set.
    pub body: String,
    /// Original bytes for resending non-text bodies verbatim.
    pub raw_bytes: Option<Vec<u8>>,
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub content_type: Option<String>,
    pub correlation_id: Option<String>,
    pub session_id: Option<String>,
    pub to: Option<String>,
    pub reply_to: Option<String>,
    pub time_to_live: Option<Duration>,
    /// Custom properties, sent as AMQP strings.
    pub application_properties: Vec<(String, String)>,
}

impl OutboundMessage {
    /// The bytes that will actually be sent.
    #[must_use]
    pub fn payload(&self) -> Vec<u8> {
        self.raw_bytes
            .clone()
            .unwrap_or_else(|| self.body.clone().into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::decode;

    #[test]
    fn to_outbound_strips_system_properties_and_keeps_body() {
        let msg = SiftMessage {
            sequence_number: 42,
            message_id: Some("original-id".into()),
            subject: Some("subject".into()),
            content_type: Some("application/json".into()),
            correlation_id: None,
            session_id: None,
            reply_to: None,
            to: None,
            enqueued_time: None,
            expires_at: None,
            time_to_live: None,
            delivery_count: Some(3),
            state: MessageState::Active,
            lock_token: Some("token".into()),
            locked_until: None,
            dead_letter_reason: Some("TTLExpired".into()),
            dead_letter_error_description: None,
            dead_letter_source: None,
            application_properties: vec![("k".into(), "v".into())],
            body: decode(br#"{"a":1}"#.to_vec()),
        };
        let out = msg.to_outbound();
        assert!(out.message_id.is_none());
        assert_eq!(out.subject.as_deref(), Some("subject"));
        assert_eq!(out.application_properties.len(), 1);
        assert!(out.raw_bytes.is_none());
        assert!(out.body.contains("\"a\": 1"));
    }

    #[test]
    fn binary_bodies_resend_raw_bytes() {
        let bytes = vec![0x00, 0xff, 0x01, 0x02, 0x03, 0x04];
        let msg = SiftMessage {
            sequence_number: 1,
            message_id: None,
            subject: None,
            content_type: None,
            correlation_id: None,
            session_id: None,
            reply_to: None,
            to: None,
            enqueued_time: None,
            expires_at: None,
            time_to_live: None,
            delivery_count: None,
            state: MessageState::Active,
            lock_token: None,
            locked_until: None,
            dead_letter_reason: None,
            dead_letter_error_description: None,
            dead_letter_source: None,
            application_properties: Vec::new(),
            body: decode(bytes.clone()),
        };
        let out = msg.to_outbound();
        assert_eq!(out.raw_bytes.as_deref(), Some(bytes.as_slice()));
        assert_eq!(out.payload(), bytes);
    }
}
