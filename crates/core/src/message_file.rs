//! Portable message templates used by the desktop import/export workflow.
//!
//! The wire format is deliberately independent of AMQP implementation types.
//! Binary payloads are base64 encoded so the resulting JSON remains portable.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::message::{OutboundMessage, SiftMessage};

const FORMAT: &str = "sift-message";
const VERSION: u32 = 1;

/// A versioned, reusable message definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFile {
    format: String,
    version: u32,
    body: MessageFileBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ttl_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    application_properties: Vec<MessageFileProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MessageFileBody {
    encoding: BodyEncoding,
    data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BodyEncoding {
    Utf8,
    Base64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MessageFileProperty {
    name: String,
    value: String,
}

#[derive(Debug, Error)]
pub enum MessageFileError {
    #[error("invalid message template JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported message template format '{0}'")]
    Format(String),
    #[error("unsupported message template version {0}")]
    Version(u32),
    #[error("message template contains invalid base64 body data")]
    InvalidBase64,
}

impl MessageFile {
    /// Build a template from a composed outbound message.
    #[must_use]
    pub fn from_outbound(message: &OutboundMessage) -> Self {
        let body = match &message.raw_bytes {
            Some(bytes) => MessageFileBody {
                encoding: BodyEncoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
            None => MessageFileBody {
                encoding: BodyEncoding::Utf8,
                data: message.body.clone(),
            },
        };
        Self::with_body(message, body)
    }

    /// Build a template from an inspected message while preserving its exact
    /// data-section bytes whenever they are available.
    #[must_use]
    pub fn from_message(message: &SiftMessage) -> Self {
        let outbound = OutboundMessage {
            message_id: message.message_id.clone(),
            ..message.to_outbound()
        };
        let body = if message.body.gzipped {
            binary_body(&message.body.bytes)
        } else if message.body.bytes.is_empty() {
            MessageFileBody {
                encoding: BodyEncoding::Utf8,
                data: message.body.text.clone().unwrap_or_default(),
            }
        } else if let Ok(text) = std::str::from_utf8(&message.body.bytes) {
            MessageFileBody {
                encoding: BodyEncoding::Utf8,
                data: text.to_owned(),
            }
        } else {
            binary_body(&message.body.bytes)
        };
        Self::with_body(&outbound, body)
    }

    fn with_body(message: &OutboundMessage, body: MessageFileBody) -> Self {
        Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            body,
            message_id: message.message_id.clone(),
            subject: message.subject.clone(),
            content_type: message.content_type.clone(),
            correlation_id: message.correlation_id.clone(),
            session_id: message.session_id.clone(),
            to: message.to.clone(),
            reply_to: message.reply_to.clone(),
            ttl_seconds: message.time_to_live.map(|duration| duration.as_secs()),
            application_properties: message
                .application_properties
                .iter()
                .map(|(name, value)| MessageFileProperty {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
        }
    }

    /// Serialize the template as stable, human-readable JSON.
    pub fn to_json(&self) -> Result<String, MessageFileError> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    /// Parse and validate a template.
    pub fn from_json(json: &str) -> Result<Self, MessageFileError> {
        let file: Self = serde_json::from_str(json)?;
        if file.format != FORMAT {
            return Err(MessageFileError::Format(file.format));
        }
        if file.version != VERSION {
            return Err(MessageFileError::Version(file.version));
        }
        if file.body.encoding == BodyEncoding::Base64
            && base64::engine::general_purpose::STANDARD
                .decode(&file.body.data)
                .is_err()
        {
            return Err(MessageFileError::InvalidBase64);
        }
        Ok(file)
    }

    /// Convert the portable representation into a message ready to edit or
    /// send.
    pub fn to_outbound(&self) -> Result<OutboundMessage, MessageFileError> {
        let (body, raw_bytes) = match self.body.encoding {
            BodyEncoding::Utf8 => (self.body.data.clone(), None),
            BodyEncoding::Base64 => (
                String::new(),
                Some(
                    base64::engine::general_purpose::STANDARD
                        .decode(&self.body.data)
                        .map_err(|_| MessageFileError::InvalidBase64)?,
                ),
            ),
        };
        Ok(OutboundMessage {
            body,
            raw_bytes,
            message_id: self.message_id.clone(),
            subject: self.subject.clone(),
            content_type: self.content_type.clone(),
            correlation_id: self.correlation_id.clone(),
            session_id: self.session_id.clone(),
            to: self.to.clone(),
            reply_to: self.reply_to.clone(),
            time_to_live: self.ttl_seconds.map(std::time::Duration::from_secs),
            application_properties: self
                .application_properties
                .iter()
                .map(|property| (property.name.clone(), property.value.clone()))
                .collect(),
        })
    }
}

fn binary_body(bytes: &[u8]) -> MessageFileBody {
    MessageFileBody {
        encoding: BodyEncoding::Base64,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::decode;
    use crate::message::MessageState;

    #[test]
    fn text_message_round_trips_all_editable_fields() {
        let message = OutboundMessage {
            body: "{\"order\":42}".to_owned(),
            message_id: Some("order-42".to_owned()),
            subject: Some("created".to_owned()),
            content_type: Some("application/json".to_owned()),
            correlation_id: Some("trace-7".to_owned()),
            session_id: Some("customer-9".to_owned()),
            to: Some("orders".to_owned()),
            reply_to: Some("order-replies".to_owned()),
            time_to_live: Some(std::time::Duration::from_secs(90)),
            application_properties: vec![("tenant".to_owned(), "northstar".to_owned())],
            ..OutboundMessage::default()
        };

        let json = MessageFile::from_outbound(&message).to_json().unwrap();
        assert!(json.contains("\"format\": \"sift-message\""));
        assert!(json.contains("\"encoding\": \"utf8\""));

        let restored = MessageFile::from_json(&json)
            .unwrap()
            .to_outbound()
            .unwrap();
        assert_eq!(restored.payload(), message.payload());
        assert_eq!(restored.message_id, message.message_id);
        assert_eq!(restored.subject, message.subject);
        assert_eq!(restored.content_type, message.content_type);
        assert_eq!(restored.correlation_id, message.correlation_id);
        assert_eq!(restored.session_id, message.session_id);
        assert_eq!(restored.to, message.to);
        assert_eq!(restored.reply_to, message.reply_to);
        assert_eq!(restored.time_to_live, message.time_to_live);
        assert_eq!(
            restored.application_properties,
            message.application_properties
        );
    }

    #[test]
    fn binary_message_uses_base64_and_round_trips() {
        let bytes = vec![0, 1, 2, 0xff, 0xfe];
        let message = OutboundMessage {
            raw_bytes: Some(bytes.clone()),
            ..OutboundMessage::default()
        };

        let json = MessageFile::from_outbound(&message).to_json().unwrap();
        assert!(json.contains("\"encoding\": \"base64\""));
        assert!(!json.contains("[\n    0,"));

        let restored = MessageFile::from_json(&json)
            .unwrap()
            .to_outbound()
            .unwrap();
        assert_eq!(restored.payload(), bytes);
    }

    #[test]
    fn inspected_message_keeps_original_json_bytes_and_id() {
        let original = br#"{"order":42}"#.to_vec();
        let message = SiftMessage {
            sequence_number: 12,
            message_id: Some("original-id".to_owned()),
            state: MessageState::Active,
            application_properties: Vec::new(),
            body: decode(original.clone()),
            subject: None,
            content_type: Some("application/json".to_owned()),
            correlation_id: None,
            session_id: None,
            reply_to: None,
            to: None,
            enqueued_time: None,
            expires_at: None,
            time_to_live: None,
            delivery_count: None,
            lock_token: None,
            locked_until: None,
            dead_letter_reason: None,
            dead_letter_error_description: None,
            dead_letter_source: None,
        };

        let restored =
            MessageFile::from_json(&MessageFile::from_message(&message).to_json().unwrap())
                .unwrap()
                .to_outbound()
                .unwrap();
        assert_eq!(restored.payload(), original);
        assert_eq!(restored.message_id.as_deref(), Some("original-id"));
    }

    #[test]
    fn rejects_unknown_versions_and_invalid_base64() {
        let wrong_version =
            r#"{"format":"sift-message","version":2,"body":{"encoding":"utf8","data":""}}"#;
        assert!(matches!(
            MessageFile::from_json(wrong_version),
            Err(MessageFileError::Version(2))
        ));

        let invalid =
            r#"{"format":"sift-message","version":1,"body":{"encoding":"base64","data":"%%%"}}"#;
        assert!(matches!(
            MessageFile::from_json(invalid),
            Err(MessageFileError::InvalidBase64)
        ));
    }
}
