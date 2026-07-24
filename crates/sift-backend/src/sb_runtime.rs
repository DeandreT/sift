//! Runtime messaging over azservicebus: one AMQP connection per namespace,
//! cached senders/receivers per entity, and a registry of peek-locked
//! messages so the UI can settle them later by lock token.

use std::collections::HashMap;

use azservicebus::core::BasicRetryPolicy;
use azservicebus::receiver::DeadLetterOptions;
use azservicebus::{
    ServiceBusClient, ServiceBusClientOptions, ServiceBusMessage, ServiceBusReceiveMode,
    ServiceBusReceiver, ServiceBusReceiverOptions, ServiceBusSender, ServiceBusSenderOptions,
    SubQueue,
};
use fe2o3_amqp_types::messaging::ApplicationProperties;
use fe2o3_amqp_types::primitives::SimpleValue;
use sift_core::body::{DecodedBody, decode};
use sift_core::connection::{NamespaceConnection, TransportType};
use sift_core::message::{MessageState, OutboundMessage, SiftMessage};

use crate::bridge::{BackendError, Disposition, EntityPath, MessageSource, ReceiveMode};

/// Peek-locked messages waiting for the UI to settle them.
struct LockedMessage {
    source: MessageSource,
    message: azservicebus::ServiceBusReceivedMessage,
}

/// One namespace's AMQP state. All operations take `&mut self` because the
/// azservicebus client and links do; callers hold this behind a tokio Mutex.
///
/// Receivers are always opened in peek-lock mode — azservicebus's
/// receive-and-delete path errors with `LockTokenNotFound` on brokers that
/// don't return a lock token for pre-settled deliveries, so sift implements
/// receive-and-delete as peek-lock followed by an immediate complete.
pub struct SbRuntime {
    client: ServiceBusClient<BasicRetryPolicy>,
    receivers: HashMap<MessageSource, ServiceBusReceiver>,
    senders: HashMap<String, ServiceBusSender>,
    locked: HashMap<String, LockedMessage>,
}

impl SbRuntime {
    pub async fn connect(conn: &NamespaceConnection) -> Result<Self, BackendError> {
        let options = ServiceBusClientOptions {
            transport_type: match conn.transport {
                TransportType::AmqpTcp => azservicebus::ServiceBusTransportType::AmqpTcp,
                TransportType::AmqpWebSockets => {
                    azservicebus::ServiceBusTransportType::AmqpWebSocket
                }
            },
            ..Default::default()
        };
        let client = ServiceBusClient::new_from_connection_string(conn.raw().expose(), options)
            .await
            .map_err(amqp_err)?;
        tracing::info!(namespace = %conn.fully_qualified_namespace, "AMQP connection established");
        Ok(Self {
            client,
            receivers: HashMap::new(),
            senders: HashMap::new(),
            locked: HashMap::new(),
        })
    }

    /// Close all links. The AMQP connection itself closes when the runtime is
    /// dropped (the client's `dispose` consumes `self`, which a shared
    /// runtime cannot give up while operations may still hold the mutex).
    pub async fn shutdown(&mut self) {
        self.locked.clear();
        for (_, receiver) in std::mem::take(&mut self.receivers) {
            let _ = receiver.dispose().await;
        }
        for (_, sender) in std::mem::take(&mut self.senders) {
            let _ = sender.dispose().await;
        }
    }

    /// Peek without consuming. `from_seq` is the starting sequence number;
    /// `None` restarts from the front. An explicit number is always passed to
    /// the AMQP receiver so results don't depend on its internal cursor (a
    /// cached receiver would otherwise keep advancing across calls).
    pub async fn peek(
        &mut self,
        source: &MessageSource,
        from_seq: Option<i64>,
        count: u32,
    ) -> Result<Vec<SiftMessage>, BackendError> {
        let receiver = self.receiver(source).await?;
        let peeked = receiver
            .peek_messages(count, Some(from_seq.unwrap_or(0)))
            .await
            .map_err(amqp_err)?;
        Ok(peeked.iter().map(from_peeked).collect())
    }

    /// Receive messages. Peek-locked ones are retained in the lock registry so
    /// they can be settled later; receive-and-delete ones are completed
    /// immediately (see the struct docs for why this isn't a native `RaD` link).
    pub async fn receive(
        &mut self,
        source: &MessageSource,
        mode: ReceiveMode,
        count: u32,
        max_wait: std::time::Duration,
    ) -> Result<Vec<SiftMessage>, BackendError> {
        // Phase 1: receive the batch, then let the receiver borrow end so the
        // per-message handling below can touch other fields of `self`.
        let batch = {
            let receiver = self.receiver(source).await?;
            receiver
                .receive_messages_with_max_wait_time(count, max_wait)
                .await
                .map_err(amqp_err)?
        };

        // Phase 2: settle (RaD) or register the lock (peek-lock). Each arm
        // borrows a disjoint field, so re-fetching the receiver here avoids
        // holding it across the `self.locked` access.
        let mut out = Vec::with_capacity(batch.len());
        for message in batch {
            let mut sift = from_received(&message);
            match mode {
                ReceiveMode::ReceiveAndDelete => {
                    let receiver = self
                        .receivers
                        .get_mut(source)
                        .expect("receiver created in phase 1");
                    receiver
                        .complete_message(&message)
                        .await
                        .map_err(amqp_err)?;
                }
                ReceiveMode::PeekLock => {
                    let token =
                        uuid::Uuid::from_bytes(*message.lock_token().as_inner()).to_string();
                    sift.lock_token = Some(token.clone());
                    self.locked.insert(
                        token,
                        LockedMessage {
                            source: source.clone(),
                            message,
                        },
                    );
                }
            }
            out.push(sift);
        }
        Ok(out)
    }

    /// Settle a peek-locked message by its lock token.
    pub async fn settle(
        &mut self,
        lock_token: &str,
        disposition: Disposition,
    ) -> Result<(), BackendError> {
        let Some(locked) = self.locked.remove(lock_token) else {
            return Err(BackendError::new(
                "the message lock is no longer held (it may have expired)",
            ));
        };
        let Some(receiver) = self.receivers.get_mut(&locked.source) else {
            return Err(BackendError::new("the receiver for this message is gone"));
        };

        let result = match &disposition {
            Disposition::Complete => receiver.complete_message(&locked.message).await,
            Disposition::Abandon => receiver.abandon_message(&locked.message, None).await,
            Disposition::Defer => receiver.defer_message(&locked.message, None).await,
            Disposition::DeadLetter {
                reason,
                description,
            } => {
                receiver
                    .dead_letter_message(
                        &locked.message,
                        DeadLetterOptions {
                            dead_letter_reason: reason.clone(),
                            dead_letter_error_description: description.clone(),
                            properties_to_modify: None,
                        },
                    )
                    .await
            }
        };
        if let Err(e) = result {
            // Put it back so the user can retry (unless the lock expired).
            self.locked.insert(lock_token.to_owned(), locked);
            return Err(amqp_err(e));
        }
        Ok(())
    }

    pub async fn send(
        &mut self,
        target: &EntityPath,
        messages: Vec<OutboundMessage>,
    ) -> Result<usize, BackendError> {
        let count = messages.len();
        let outgoing = build_messages(&messages)?;
        let sender = self.sender(target).await?;
        sender.send_messages(outgoing).await.map_err(amqp_err)?;
        Ok(count)
    }

    /// Schedule messages for future enqueue; returns their sequence numbers.
    pub async fn schedule(
        &mut self,
        target: &EntityPath,
        messages: Vec<OutboundMessage>,
        enqueue_at: time::OffsetDateTime,
    ) -> Result<Vec<i64>, BackendError> {
        let outgoing = build_messages(&messages)?;
        let sender = self.sender(target).await?;
        sender
            .schedule_messages(outgoing, enqueue_at)
            .await
            .map_err(amqp_err)
    }

    pub async fn cancel_scheduled(
        &mut self,
        target: &EntityPath,
        sequence_number: i64,
    ) -> Result<(), BackendError> {
        let sender = self.sender(target).await?;
        sender
            .cancel_scheduled_message(sequence_number)
            .await
            .map_err(amqp_err)
    }

    /// Retrieve deferred messages by sequence number. They come back locked,
    /// so they are registered like any other peek-lock receive.
    pub async fn receive_deferred(
        &mut self,
        source: &MessageSource,
        sequence_numbers: Vec<i64>,
    ) -> Result<Vec<SiftMessage>, BackendError> {
        let received = {
            let receiver = self.receiver(source).await?;
            receiver
                .receive_deferred_messages(sequence_numbers)
                .await
                .map_err(amqp_err)?
        };
        let mut out = Vec::with_capacity(received.len());
        for message in received {
            let mut sift = from_received(&message);
            let token = uuid::Uuid::from_bytes(*message.lock_token().as_inner()).to_string();
            sift.lock_token = Some(token.clone());
            self.locked.insert(
                token,
                LockedMessage {
                    source: source.clone(),
                    message,
                },
            );
            out.push(sift);
        }
        Ok(out)
    }

    /// Get (or open) the sender for a queue or topic.
    async fn sender(&mut self, target: &EntityPath) -> Result<&mut ServiceBusSender, BackendError> {
        let path = match target {
            EntityPath::Queue(name) | EntityPath::Topic(name) => name.clone(),
            other => {
                return Err(BackendError::new(format!(
                    "cannot send to a {}",
                    other.kind()
                )));
            }
        };
        if !self.senders.contains_key(&path) {
            let sender = self
                .client
                .create_sender(&path, ServiceBusSenderOptions::default())
                .await
                .map_err(amqp_err)?;
            self.senders.insert(path.clone(), sender);
        }
        Ok(self
            .senders
            .get_mut(&path)
            .expect("sender was just inserted"))
    }

    /// Get (or open) the peek-lock receiver for a source. Receive-and-delete
    /// semantics are layered on top in [`Self::receive`].
    async fn receiver(
        &mut self,
        source: &MessageSource,
    ) -> Result<&mut ServiceBusReceiver, BackendError> {
        if !self.receivers.contains_key(source) {
            let options = ServiceBusReceiverOptions {
                receive_mode: ServiceBusReceiveMode::PeekLock,
                sub_queue: if source.dead_letter {
                    SubQueue::DeadLetter
                } else {
                    SubQueue::None
                },
                ..Default::default()
            };
            let receiver = match &source.entity {
                EntityPath::Queue(name) => self
                    .client
                    .create_receiver_for_queue(name.clone(), options)
                    .await
                    .map_err(amqp_err)?,
                EntityPath::Subscription { topic, name } => self
                    .client
                    .create_receiver_for_subscription(topic, name, options)
                    .await
                    .map_err(amqp_err)?,
                other => {
                    return Err(BackendError::new(format!(
                        "cannot receive from a {}",
                        other.kind()
                    )));
                }
            };
            self.receivers.insert(source.clone(), receiver);
        }
        Ok(self
            .receivers
            .get_mut(source)
            .expect("receiver was just inserted"))
    }
}

// ---- conversions -------------------------------------------------------------

fn amqp_err(e: impl std::fmt::Display) -> BackendError {
    BackendError::new(e.to_string())
}

fn decoded_body(
    bytes: Option<&[u8]>,
    raw_body: &fe2o3_amqp_types::messaging::Body<fe2o3_amqp_types::primitives::Value>,
) -> DecodedBody {
    match bytes {
        Some(bytes) => decode(bytes.to_vec()),
        // Value/Sequence bodies: render a debug preview.
        None => DecodedBody::amqp_value(format!("{raw_body:#?}")),
    }
}

fn app_properties(props: Option<&ApplicationProperties>) -> Vec<(String, String)> {
    props
        .map(|p| {
            p.0.iter()
                .map(|(k, v)| (k.clone(), simple_value_to_string(v)))
                .collect()
        })
        .unwrap_or_default()
}

fn simple_value_to_string(value: &SimpleValue) -> String {
    match value {
        SimpleValue::String(s) => s.clone(),
        SimpleValue::Bool(b) => b.to_string(),
        SimpleValue::Int(i) => i.to_string(),
        SimpleValue::Long(l) => l.to_string(),
        SimpleValue::Double(d) => (**d).to_string(),
        SimpleValue::Float(f) => (**f).to_string(),
        SimpleValue::Ubyte(v) => v.to_string(),
        SimpleValue::Ushort(v) => v.to_string(),
        SimpleValue::Uint(v) => v.to_string(),
        SimpleValue::Ulong(v) => v.to_string(),
        SimpleValue::Byte(v) => v.to_string(),
        SimpleValue::Short(v) => v.to_string(),
        other => format!("{other:?}"),
    }
}

fn message_state(state: azservicebus::ServiceBusMessageState) -> MessageState {
    match state {
        azservicebus::ServiceBusMessageState::Active => MessageState::Active,
        azservicebus::ServiceBusMessageState::Deferred => MessageState::Deferred,
        azservicebus::ServiceBusMessageState::Scheduled => MessageState::Scheduled,
    }
}

fn from_peeked(m: &azservicebus::ServiceBusPeekedMessage) -> SiftMessage {
    SiftMessage {
        sequence_number: m.sequence_number(),
        message_id: m.message_id().map(std::borrow::Cow::into_owned),
        subject: m.subject().map(str::to_owned),
        content_type: m.content_type().map(str::to_owned),
        correlation_id: m.correlation_id().map(std::borrow::Cow::into_owned),
        session_id: m.session_id().map(str::to_owned),
        reply_to: m.reply_to().map(str::to_owned),
        to: m.to().map(str::to_owned),
        enqueued_time: Some(m.enqueued_time()),
        expires_at: Some(m.expires_at()),
        time_to_live: m.time_to_live(),
        delivery_count: m.delivery_count(),
        state: message_state(m.state()),
        lock_token: None,
        locked_until: None,
        dead_letter_reason: m.dead_letter_reason().map(str::to_owned),
        dead_letter_error_description: m.dead_letter_error_description().map(str::to_owned),
        dead_letter_source: m.dead_letter_source().map(str::to_owned),
        application_properties: app_properties(m.application_properties()),
        body: decoded_body(m.body().ok(), &m.raw_amqp_message().body),
    }
}

fn from_received(m: &azservicebus::ServiceBusReceivedMessage) -> SiftMessage {
    SiftMessage {
        sequence_number: m.sequence_number(),
        message_id: m.message_id().map(std::borrow::Cow::into_owned),
        subject: m.subject().map(str::to_owned),
        content_type: m.content_type().map(str::to_owned),
        correlation_id: m.correlation_id().map(std::borrow::Cow::into_owned),
        session_id: m.session_id().map(str::to_owned),
        reply_to: m.reply_to().map(str::to_owned),
        to: m.to().map(str::to_owned),
        enqueued_time: Some(m.enqueued_time()),
        expires_at: Some(m.expires_at()),
        time_to_live: m.time_to_live(),
        delivery_count: m.delivery_count(),
        state: message_state(m.state()),
        lock_token: None,
        locked_until: m.locked_until(),
        dead_letter_reason: m.dead_letter_reason().map(str::to_owned),
        dead_letter_error_description: m.dead_letter_error_description().map(str::to_owned),
        dead_letter_source: m.dead_letter_source().map(str::to_owned),
        application_properties: app_properties(m.application_properties()),
        body: decoded_body(m.body().ok(), &m.raw_amqp_message().body),
    }
}

fn build_messages(messages: &[OutboundMessage]) -> Result<Vec<ServiceBusMessage>, BackendError> {
    messages.iter().map(to_service_bus_message).collect()
}

fn to_service_bus_message(out: &OutboundMessage) -> Result<ServiceBusMessage, BackendError> {
    let mut message = ServiceBusMessage::new(out.payload());

    if let Some(id) = &out.message_id {
        message
            .set_message_id(id.clone())
            .map_err(|e| BackendError::new(format!("invalid message id: {e}")))?;
    }
    if let Some(session) = &out.session_id {
        message
            .set_session_id(Some(session.clone()))
            .map_err(|e| BackendError::new(format!("invalid session id: {e}")))?;
    }
    if let Some(ttl) = out.time_to_live {
        message
            .set_time_to_live(Some(ttl))
            .map_err(|e| BackendError::new(format!("invalid time to live: {e}")))?;
    }
    message.set_subject(out.subject.clone());
    message.set_content_type(out.content_type.clone());
    message.set_correlation_id(out.correlation_id.clone());
    message.set_to(out.to.clone());
    message.set_reply_to(out.reply_to.clone());

    if !out.application_properties.is_empty() {
        let mut props = ApplicationProperties::default();
        for (key, value) in &out.application_properties {
            props
                .0
                .insert(key.clone(), SimpleValue::String(value.clone()));
        }
        *message.application_properties_mut() = Some(props);
    }
    Ok(message)
}
