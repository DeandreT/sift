use std::collections::HashMap;
use std::time::Duration;

use egui_dock::{DockArea, DockState};
use sift_backend::{
    Disposition, EntityInfo, EntityPath, MessageSource, NamespaceId, ReceiveMode, SessionSnapshot,
};
use sift_core::body::{DecodedBody, decode};
use sift_core::message::{MessageState, SiftMessage};
use sift_mgmt::{
    EntityRuntimeInfo, MessageCountDetails, NamespaceInfo, QueueInfo, QueueProperties, RuleFilter,
    RuleInfo, RuleProperties, SubscriptionInfo, SubscriptionProperties, TopicInfo, TopicProperties,
};
use sift_ui::icons::{Icon, icon};
use sift_ui::state::{
    AppAction, Connection, DashboardState, EntityPage, EntityTabState, EntityTree, Loadable,
    ScopedEntity, TreeFilter,
};
use sift_ui::ui::{tabs::TabId, tabs::TabViewerCtx, tree_panel};
use time::{OffsetDateTime, macros::datetime};
use uuid::Uuid;

const DEMO_NAMESPACE: NamespaceId = Uuid::from_u128(0x54d9b9a4_2c76_4e72_9426_19a2f01a91c4);
const SIMULATION_INTERVAL: f64 = 3.5;
const PEEK_BATCH: u32 = 20;

pub struct DemoApp {
    connections: Vec<Connection>,
    filter: TreeFilter,
    dashboard: DashboardState,
    entities: HashMap<ScopedEntity, EntityTabState>,
    dock: DockState<TabId>,
    messages: HashMap<MessageSource, Vec<SiftMessage>>,
    next_sequence: i64,
    generated: u64,
    paused: bool,
    tree_visible: bool,
    layout_initialized: bool,
    last_emit_at: f64,
    frame_time: f64,
    notice: Option<(String, f64)>,
}

impl DemoApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        sift_ui::icons::install(&cc.egui_ctx);

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::from_rgb(19, 24, 27);
        visuals.extreme_bg_color = egui::Color32::from_rgb(13, 17, 19);
        visuals.selection.bg_fill = egui::Color32::from_rgb(35, 111, 91);
        visuals.hyperlink_color = egui::Color32::from_rgb(88, 198, 167);
        cc.egui_ctx.set_visuals(visuals);

        Self::seeded()
    }

    #[allow(clippy::too_many_lines)] // Static fixture assembly is clearest in one place.
    fn seeded() -> Self {
        let queues = vec![
            queue("orders", 14, 2, 1, false),
            queue("payment-capture", 6, 0, 0, false),
            queue("billing-retry", 3, 4, 0, false),
            queue("session-work", 8, 0, 0, true),
        ];
        let topics = vec![topic("order-events", 3), topic("inventory-events", 2)];
        let order_subscriptions = vec![
            subscription("order-events", "fulfillment", 9, 1, false),
            subscription("order-events", "analytics", 18, 0, false),
            subscription("order-events", "notifications", 4, 2, false),
        ];
        let inventory_subscriptions = vec![
            subscription("inventory-events", "allocation", 7, 0, false),
            subscription("inventory-events", "replenishment", 2, 1, false),
        ];

        let mut subscriptions = HashMap::new();
        subscriptions.insert(
            "order-events".to_owned(),
            Loadable::Loaded(order_subscriptions.clone()),
        );
        subscriptions.insert(
            "inventory-events".to_owned(),
            Loadable::Loaded(inventory_subscriptions),
        );

        let mut rules = HashMap::new();
        for subscription in &order_subscriptions {
            let key = (
                subscription.properties.topic.clone(),
                subscription.properties.name.clone(),
            );
            rules.insert(
                key,
                Loadable::Loaded(vec![rule(
                    "order-events",
                    &subscription.properties.name,
                    "$Default",
                    RuleFilter::True,
                )]),
            );
        }
        rules.insert(
            ("order-events".to_owned(), "analytics".to_owned()),
            Loadable::Loaded(vec![
                rule(
                    "order-events",
                    "analytics",
                    "completed-orders",
                    RuleFilter::Sql {
                        expression: "eventType = 'order.completed'".to_owned(),
                    },
                ),
                rule(
                    "order-events",
                    "analytics",
                    "high-value",
                    RuleFilter::Sql {
                        expression: "total >= 500".to_owned(),
                    },
                ),
            ]),
        );

        let connection = Connection {
            profile_id: DEMO_NAMESPACE,
            name: "northstar-demo".to_owned(),
            info: Some(NamespaceInfo {
                name: "northstar-demo".to_owned(),
                alias: Some("Browser simulation".to_owned()),
                namespace_type: Some("Messaging".to_owned()),
                messaging_sku: Some("Standard".to_owned()),
                messaging_units: Some(1),
                created_time: Some(datetime!(2025-11-03 09:30 UTC)),
                modified_time: Some(datetime!(2026-07-24 16:00 UTC)),
            }),
            tree: EntityTree {
                queues: Loadable::Loaded(queues.clone()),
                topics: Loadable::Loaded(topics),
                subscriptions,
                rules,
            },
        };

        let mut messages = HashMap::new();
        seed_source(&mut messages, EntityPath::Queue("orders".to_owned()), 12, 0);
        seed_source(
            &mut messages,
            EntityPath::Queue("payment-capture".to_owned()),
            6,
            100,
        );
        seed_source(
            &mut messages,
            EntityPath::Queue("billing-retry".to_owned()),
            3,
            200,
        );
        seed_source(
            &mut messages,
            EntityPath::Queue("session-work".to_owned()),
            8,
            300,
        );
        seed_source(
            &mut messages,
            EntityPath::Subscription {
                topic: "order-events".to_owned(),
                name: "fulfillment".to_owned(),
            },
            9,
            400,
        );
        seed_source(
            &mut messages,
            EntityPath::Subscription {
                topic: "order-events".to_owned(),
                name: "analytics".to_owned(),
            },
            18,
            500,
        );
        seed_source(
            &mut messages,
            EntityPath::Subscription {
                topic: "order-events".to_owned(),
                name: "notifications".to_owned(),
            },
            4,
            600,
        );

        seed_dead_letters(
            &mut messages,
            EntityPath::Queue("orders".to_owned()),
            2,
            700,
        );
        seed_dead_letters(
            &mut messages,
            EntityPath::Queue("billing-retry".to_owned()),
            4,
            710,
        );
        seed_dead_letters(
            &mut messages,
            EntityPath::Subscription {
                topic: "order-events".to_owned(),
                name: "fulfillment".to_owned(),
            },
            1,
            720,
        );

        let orders = ScopedEntity::new(DEMO_NAMESPACE, EntityPath::Queue("orders".to_owned()));
        let orders_info = EntityInfo::Queue(queues[0].clone());
        let mut orders_state = EntityTabState::new(PEEK_BATCH);
        orders_state.info = Loadable::Loaded(orders_info);
        orders_state.page = EntityPage::Messages;
        orders_state.main.rows = messages
            .get(&source(orders.path.clone(), false))
            .cloned()
            .unwrap_or_default();
        orders_state.main.selected = Some(0);
        orders_state.dead_letter.rows = messages
            .get(&source(orders.path.clone(), true))
            .cloned()
            .unwrap_or_default();

        let mut entities = HashMap::new();
        entities.insert(orders.clone(), orders_state);

        let mut dock = DockState::new(vec![TabId::Dashboard, TabId::Entity(orders.clone())]);
        if let Some(location) = dock.find_tab(&TabId::Entity(orders)) {
            let _ = dock.set_active_tab(location);
        }

        let mut app = Self {
            connections: vec![connection],
            filter: TreeFilter::default(),
            dashboard: DashboardState::default(),
            entities,
            dock,
            messages,
            next_sequence: 1_000,
            generated: 0,
            paused: false,
            tree_visible: true,
            layout_initialized: false,
            last_emit_at: 0.0,
            frame_time: 0.0,
            notice: None,
        };
        app.sync_counts();
        app
    }

    fn tick_simulation(&mut self, ctx: &egui::Context) {
        self.frame_time = ctx.input(|input| input.time);
        if self.last_emit_at == 0.0 {
            self.last_emit_at = self.frame_time;
        }
        if !self.paused && self.frame_time - self.last_emit_at >= SIMULATION_INTERVAL {
            self.last_emit_at = self.frame_time;
            self.emit_message();
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, until)| self.frame_time >= *until)
        {
            self.notice = None;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn emit_message(&mut self) {
        let path = match self.generated % 5 {
            0 | 3 => EntityPath::Queue("orders".to_owned()),
            1 => EntityPath::Queue("payment-capture".to_owned()),
            2 => EntityPath::Subscription {
                topic: "order-events".to_owned(),
                name: "fulfillment".to_owned(),
            },
            _ => EntityPath::Subscription {
                topic: "order-events".to_owned(),
                name: "analytics".to_owned(),
            },
        };
        let dead_letter = self.generated > 0 && self.generated.is_multiple_of(11);
        let message = sample_message(self.next_sequence, path.name(), dead_letter);
        self.next_sequence += 1;
        self.generated += 1;

        let message_source = source(path.clone(), dead_letter);
        self.messages
            .entry(message_source.clone())
            .or_default()
            .push(message.clone());

        let scoped = ScopedEntity::new(DEMO_NAMESPACE, path);
        if let Some(state) = self.entities.get_mut(&scoped) {
            let view = state.view_mut(dead_letter);
            if !view.rows.is_empty() {
                view.rows.push(message);
                if view.rows.len() > 200 {
                    view.rows.remove(0);
                    view.selected = view.selected.and_then(|selected| selected.checked_sub(1));
                }
            }
        }
        self.sync_counts();
    }

    fn run_action(&mut self, action: AppAction) {
        match action {
            AppAction::OpenEntity(scoped) | AppAction::DockEntity(scoped) => {
                self.open_entity(scoped);
            }
            AppAction::OpenDashboard => {
                let tab = TabId::Dashboard;
                if let Some(location) = self.dock.find_tab(&tab) {
                    let _ = self.dock.set_active_tab(location);
                } else {
                    self.dock.push_to_focused_leaf(tab);
                }
            }
            AppAction::PeekMessages {
                source,
                from_seq,
                count,
                ..
            } => self.peek(&source, from_seq, count),
            AppAction::ReceiveMessages {
                source,
                mode,
                count,
                ..
            } => self.receive(&source, mode, count),
            AppAction::Settle {
                source,
                lock_token,
                disposition,
                ..
            } => self.settle(&source, &lock_token, disposition),
            AppAction::RequestPurge { source, .. } => self.purge(&source),
            AppAction::ResubmitAll { source, .. } => self.resubmit_all(&source),
            AppAction::CancelScheduled {
                target,
                sequence_number,
                ..
            } => self.cancel_scheduled(target, sequence_number),
            AppAction::ReceiveDeferred {
                source,
                sequence_numbers,
                ..
            } => self.receive_deferred(&source, &sequence_numbers),
            AppAction::BrowseSession {
                source,
                session_id,
                count,
                ..
            } => self.browse_session(source, session_id, count),
            AppAction::OpenSendDialog {
                target, prefill, ..
            } => self.send_sample(target, prefill.as_deref()),
            AppAction::RefreshEntity(scoped) => self.refresh_entity(&scoped),
            AppAction::UpdateEntity { info, .. } => {
                self.update_entity(*info);
                self.note("Entity status updated in the simulation");
            }
            AppAction::SetDashboardAutoRefresh(mode) => {
                self.dashboard.auto_refresh = mode;
            }
            AppAction::RefreshDashboard
            | AppAction::LoadQueues(_)
            | AppAction::LoadTopics(_)
            | AppAction::LoadSubscriptions { .. }
            | AppAction::LoadRules { .. } => {
                self.sync_counts();
                self.note("Simulation data refreshed");
            }
            AppAction::RefreshTree(_) => {
                self.sync_counts();
                self.note("Namespace tree refreshed");
            }
            AppAction::PopOutEntity(_) => {
                self.note("Detached windows are available in the desktop application");
            }
            AppAction::OpenConnectDialog
            | AppAction::Disconnect(_)
            | AppAction::ImportLegacyProfiles
            | AppAction::OpenCreateDialog { .. }
            | AppAction::RequestDelete(_)
            | AppAction::CancelOp(_)
            | AppAction::ExportNamespace(_)
            | AppAction::ImportNamespace { .. }
            | AppAction::SaveMessageBody(_)
            | AppAction::SaveMessageTemplate(_) => {
                self.note("This command is disabled in the public simulation");
            }
        }
    }

    fn open_entity(&mut self, scoped: ScopedEntity) {
        let info = self.lookup_info(&scoped.path);
        let state = self
            .entities
            .entry(scoped.clone())
            .or_insert_with(|| EntityTabState::new(PEEK_BATCH));
        if let Some(info) = info {
            state.info = Loadable::Loaded(info);
        }

        let tab = TabId::Entity(scoped);
        if let Some(location) = self.dock.find_tab(&tab) {
            let _ = self.dock.set_active_tab(location);
        } else {
            self.dock.push_to_focused_leaf(tab);
        }
    }

    fn peek(&mut self, message_source: &MessageSource, from_seq: Option<i64>, count: u32) {
        let rows: Vec<SiftMessage> = self
            .messages
            .get(message_source)
            .into_iter()
            .flatten()
            .filter(|message| from_seq.is_none_or(|sequence| message.sequence_number >= sequence))
            .take(count as usize)
            .cloned()
            .collect();
        let view = self.message_view_mut(message_source);
        if from_seq.is_some() {
            view.rows.extend(rows);
        } else {
            view.rows = rows;
            view.selected = None;
        }
        view.loading = false;
        view.error = None;
        self.note("Peek completed");
    }

    fn receive(&mut self, message_source: &MessageSource, mode: ReceiveMode, count: u32) {
        let take = count as usize;
        let mut rows = self
            .messages
            .get(message_source)
            .map(|messages| messages.iter().take(take).cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        match mode {
            ReceiveMode::PeekLock => {
                for message in &mut rows {
                    message.lock_token = Some(format!("demo-lock-{}", message.sequence_number));
                    message.locked_until =
                        Some(OffsetDateTime::now_utc() + time::Duration::minutes(1));
                    message.delivery_count = Some(message.delivery_count.unwrap_or(0) + 1);
                }
            }
            ReceiveMode::ReceiveAndDelete => {
                if let Some(messages) = self.messages.get_mut(message_source) {
                    messages.drain(..rows.len());
                }
            }
        }

        let view = self.message_view_mut(message_source);
        view.rows = rows;
        view.selected = None;
        view.loading = false;
        view.error = None;
        self.sync_counts();
        self.note(match mode {
            ReceiveMode::PeekLock => "Messages received with simulated locks",
            ReceiveMode::ReceiveAndDelete => "Messages received and removed",
        });
    }

    fn settle(
        &mut self,
        message_source: &MessageSource,
        lock_token: &str,
        disposition: Disposition,
    ) {
        let selected = self
            .message_view_mut(message_source)
            .rows
            .iter()
            .find(|message| message.lock_token.as_deref() == Some(lock_token))
            .cloned();
        let Some(mut selected) = selected else {
            self.note("The simulated lock has expired");
            return;
        };

        if disposition == Disposition::Abandon {
            if let Some(row) = self
                .message_view_mut(message_source)
                .rows
                .iter_mut()
                .find(|message| message.lock_token.as_deref() == Some(lock_token))
            {
                row.lock_token = None;
                row.locked_until = None;
            }
            self.note("Abandoned the message");
            return;
        }

        if let Some(messages) = self.messages.get_mut(message_source)
            && let Some(index) = messages
                .iter()
                .position(|message| message.sequence_number == selected.sequence_number)
        {
            messages.remove(index);
        }

        let verb = disposition.verb();
        match disposition {
            Disposition::Defer => {
                selected.state = MessageState::Deferred;
                selected.lock_token = None;
                selected.locked_until = None;
                self.messages
                    .entry(message_source.clone())
                    .or_default()
                    .push(selected);
            }
            Disposition::DeadLetter {
                reason,
                description,
            } => {
                selected.lock_token = None;
                selected.locked_until = None;
                selected.dead_letter_reason = reason.or_else(|| Some("sift demo".to_owned()));
                selected.dead_letter_error_description = description;
                selected.dead_letter_source = Some(message_source.entity.to_string());
                self.messages
                    .entry(source(message_source.entity.clone(), true))
                    .or_default()
                    .push(selected);
            }
            Disposition::Complete | Disposition::Abandon => {}
        }
        self.message_view_mut(message_source)
            .remove_by_lock_token(lock_token);
        self.sync_counts();
        self.note(format!("{verb} the message"));
    }

    fn purge(&mut self, message_source: &MessageSource) {
        self.messages
            .entry(message_source.clone())
            .or_default()
            .clear();
        let view = self.message_view_mut(message_source);
        view.rows.clear();
        view.selected = None;
        self.sync_counts();
        self.note("Purged the simulated source");
    }

    fn resubmit_all(&mut self, dead_letter_source: &MessageSource) {
        let mut messages = self.messages.remove(dead_letter_source).unwrap_or_default();
        for message in &mut messages {
            message.dead_letter_reason = None;
            message.dead_letter_error_description = None;
            message.dead_letter_source = None;
            message.lock_token = None;
            message.locked_until = None;
            message.state = MessageState::Active;
        }
        let count = messages.len();
        self.messages
            .entry(source(dead_letter_source.entity.clone(), false))
            .or_default()
            .append(&mut messages);
        let view = self.message_view_mut(dead_letter_source);
        view.rows.clear();
        view.selected = None;
        self.sync_counts();
        self.note(format!("Resubmitted {count} simulated messages"));
    }

    fn cancel_scheduled(&mut self, target: EntityPath, sequence_number: i64) {
        let message_source = source(target, false);
        if let Some(messages) = self.messages.get_mut(&message_source) {
            messages.retain(|message| message.sequence_number != sequence_number);
        }
        self.message_view_mut(&message_source)
            .rows
            .retain(|message| message.sequence_number != sequence_number);
        self.sync_counts();
        self.note("Cancelled the scheduled message");
    }

    fn receive_deferred(&mut self, message_source: &MessageSource, sequences: &[i64]) {
        let mut rows: Vec<SiftMessage> = self
            .messages
            .get(message_source)
            .into_iter()
            .flatten()
            .filter(|message| sequences.contains(&message.sequence_number))
            .cloned()
            .collect();
        for message in &mut rows {
            message.lock_token = Some(format!("demo-lock-{}", message.sequence_number));
            message.locked_until = Some(OffsetDateTime::now_utc() + time::Duration::minutes(1));
        }
        let view = self.message_view_mut(message_source);
        view.rows = rows;
        view.selected = None;
        self.note("Retrieved deferred messages");
    }

    fn browse_session(
        &mut self,
        message_source: MessageSource,
        session_id: Option<String>,
        count: u32,
    ) {
        let wanted = session_id.unwrap_or_else(|| "order-1042".to_owned());
        let messages: Vec<SiftMessage> = self
            .messages
            .get(&message_source)
            .into_iter()
            .flatten()
            .filter(|message| message.session_id.as_deref() == Some(wanted.as_str()))
            .take(count as usize)
            .cloned()
            .collect();
        let scoped = ScopedEntity::new(DEMO_NAMESPACE, message_source.entity);
        let state = self
            .entities
            .entry(scoped)
            .or_insert_with(|| EntityTabState::new(PEEK_BATCH));
        state.sessions.loading = false;
        state.sessions.error = None;
        state.sessions.snapshot = Some(SessionSnapshot {
            session_id: wanted,
            state: Some(DecodedBody::amqp_value(
                "{ status: \"processing\", attempt: 2 }".to_owned(),
            )),
            messages,
        });
        self.note("Accepted the simulated session");
    }

    fn send_sample(
        &mut self,
        target: EntityPath,
        prefill: Option<&sift_core::message::OutboundMessage>,
    ) {
        let destination = match target {
            EntityPath::Topic(topic) => EntityPath::Subscription {
                topic,
                name: "fulfillment".to_owned(),
            },
            other => other,
        };
        let mut message = sample_message(self.next_sequence, destination.name(), false);
        self.next_sequence += 1;
        if let Some(prefill) = prefill {
            message.body = prefill
                .raw_bytes
                .clone()
                .map_or_else(|| decode(prefill.body.clone().into_bytes()), decode);
            message.subject.clone_from(&prefill.subject);
            message.content_type.clone_from(&prefill.content_type);
            message
                .application_properties
                .clone_from(&prefill.application_properties);
        }
        self.messages
            .entry(source(destination, false))
            .or_default()
            .push(message);
        self.sync_counts();
        self.note("Sent a message into the simulation");
    }

    fn refresh_entity(&mut self, scoped: &ScopedEntity) {
        if let Some(info) = self.lookup_info(&scoped.path)
            && let Some(state) = self.entities.get_mut(scoped)
        {
            state.info = Loadable::Loaded(info);
        }
        self.note("Entity details refreshed");
    }

    fn update_entity(&mut self, info: EntityInfo) {
        let Some(connection) = self.connections.first_mut() else {
            return;
        };
        match &info {
            EntityInfo::Queue(updated) => {
                if let Loadable::Loaded(queues) = &mut connection.tree.queues
                    && let Some(queue) = queues
                        .iter_mut()
                        .find(|queue| queue.properties.name == updated.properties.name)
                {
                    *queue = updated.clone();
                }
            }
            EntityInfo::Topic(updated) => {
                if let Loadable::Loaded(topics) = &mut connection.tree.topics
                    && let Some(topic) = topics
                        .iter_mut()
                        .find(|topic| topic.properties.name == updated.properties.name)
                {
                    *topic = updated.clone();
                }
            }
            EntityInfo::Subscription(updated) => {
                if let Some(Loadable::Loaded(subscriptions)) = connection
                    .tree
                    .subscriptions
                    .get_mut(&updated.properties.topic)
                    && let Some(subscription) = subscriptions
                        .iter_mut()
                        .find(|item| item.properties.name == updated.properties.name)
                {
                    *subscription = updated.clone();
                }
            }
            EntityInfo::Rule(updated) => {
                let key = (
                    updated.properties.topic.clone(),
                    updated.properties.subscription.clone(),
                );
                if let Some(Loadable::Loaded(rules)) = connection.tree.rules.get_mut(&key)
                    && let Some(rule) = rules
                        .iter_mut()
                        .find(|item| item.properties.name == updated.properties.name)
                {
                    *rule = updated.clone();
                }
            }
        }
        let scoped = ScopedEntity::new(DEMO_NAMESPACE, info.path());
        if let Some(state) = self.entities.get_mut(&scoped) {
            state.info = Loadable::Loaded(info);
        }
    }

    fn message_view_mut(
        &mut self,
        message_source: &MessageSource,
    ) -> &mut sift_ui::state::MessagesView {
        let scoped = ScopedEntity::new(DEMO_NAMESPACE, message_source.entity.clone());
        self.entities
            .entry(scoped)
            .or_insert_with(|| EntityTabState::new(PEEK_BATCH))
            .view_mut(message_source.dead_letter)
    }

    fn lookup_info(&self, path: &EntityPath) -> Option<EntityInfo> {
        let tree = &self.connections.first()?.tree;
        match path {
            EntityPath::Queue(name) => match &tree.queues {
                Loadable::Loaded(queues) => queues
                    .iter()
                    .find(|queue| queue.properties.name == *name)
                    .cloned()
                    .map(EntityInfo::Queue),
                _ => None,
            },
            EntityPath::Topic(name) => match &tree.topics {
                Loadable::Loaded(topics) => topics
                    .iter()
                    .find(|topic| topic.properties.name == *name)
                    .cloned()
                    .map(EntityInfo::Topic),
                _ => None,
            },
            EntityPath::Subscription { topic, name } => match tree.subscriptions.get(topic) {
                Some(Loadable::Loaded(subscriptions)) => subscriptions
                    .iter()
                    .find(|subscription| subscription.properties.name == *name)
                    .cloned()
                    .map(EntityInfo::Subscription),
                _ => None,
            },
            EntityPath::Rule {
                topic,
                subscription,
                name,
            } => match tree.rules.get(&(topic.clone(), subscription.clone())) {
                Some(Loadable::Loaded(rules)) => rules
                    .iter()
                    .find(|rule| rule.properties.name == *name)
                    .cloned()
                    .map(EntityInfo::Rule),
                _ => None,
            },
        }
    }

    fn sync_counts(&mut self) {
        let Some(connection) = self.connections.first_mut() else {
            return;
        };
        if let Loadable::Loaded(queues) = &mut connection.tree.queues {
            for queue in queues {
                queue.runtime = runtime_for(
                    &self.messages,
                    &EntityPath::Queue(queue.properties.name.clone()),
                );
            }
        }
        for subscriptions in connection.tree.subscriptions.values_mut() {
            if let Loadable::Loaded(subscriptions) = subscriptions {
                for subscription in subscriptions {
                    subscription.runtime = runtime_for(
                        &self.messages,
                        &EntityPath::Subscription {
                            topic: subscription.properties.topic.clone(),
                            name: subscription.properties.name.clone(),
                        },
                    );
                }
            }
        }

        let open: Vec<ScopedEntity> = self.entities.keys().cloned().collect();
        for scoped in open {
            if let Some(info) = self.lookup_info(&scoped.path)
                && let Some(state) = self.entities.get_mut(&scoped)
            {
                state.info = Loadable::Loaded(info);
            }
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), self.frame_time + 4.0));
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("sift");
            ui.label(egui::RichText::new("interactive demo").weak());
            ui.separator();
            ui.colored_label(
                egui::Color32::from_rgb(88, 198, 167),
                format!("{} in-memory", icon(Icon::Activity)),
            );
            if ui
                .button(icon(if self.tree_visible {
                    Icon::PanelLeftClose
                } else {
                    Icon::PanelLeftOpen
                }))
                .on_hover_text(if self.tree_visible {
                    "Hide namespace tree"
                } else {
                    "Show namespace tree"
                })
                .clicked()
            {
                self.tree_visible = !self.tree_visible;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(format!("{} Reset", icon(Icon::RotateCcw)))
                    .clicked()
                {
                    *self = Self::seeded();
                    self.note("Simulation reset");
                }
                let (glyph, label) = if self.paused {
                    (Icon::Play, "Resume")
                } else {
                    (Icon::Pause, "Pause")
                };
                if ui.button(format!("{} {label}", icon(glyph))).clicked() {
                    self.paused = !self.paused;
                    self.last_emit_at = self.frame_time;
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let state = if self.paused { "paused" } else { "running" };
            ui.label(format!("Simulation {state}"));
            ui.separator();
            ui.label(format!("{} generated", self.generated));
            if let Some((notice, _)) = &self.notice {
                ui.separator();
                ui.label(egui::RichText::new(notice).weak());
            }
        });
    }
}

impl eframe::App for DemoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.tick_simulation(ui.ctx());
        if !self.layout_initialized {
            self.tree_visible = ui.available_width() >= 720.0;
            self.layout_initialized = true;
        }
        let mut actions = Vec::new();

        egui::Panel::top("demo_top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("demo_status").show(ui, |ui| self.status_bar(ui));
        if self.tree_visible {
            egui::Panel::left("demo_tree")
                .resizable(true)
                .default_size(255.0)
                .size_range(175.0..=430.0)
                .show(ui, |ui| {
                    tree_panel::show(ui, &self.connections, &mut self.filter, &mut actions);
                });
        }

        let mut viewer = TabViewerCtx {
            connections: &self.connections,
            dashboard: &mut self.dashboard,
            entities: &mut self.entities,
            peek_batch: PEEK_BATCH,
            actions: &mut actions,
        };
        DockArea::new(&mut self.dock).show_inside(ui, &mut viewer);

        for action in actions {
            self.run_action(action);
        }
    }
}

fn queue(
    name: &str,
    active: i64,
    dead_letter: i64,
    scheduled: i64,
    requires_session: bool,
) -> QueueInfo {
    QueueInfo {
        properties: QueueProperties {
            name: name.to_owned(),
            requires_session,
            user_metadata: Some("Managed by the sift browser simulation".to_owned()),
            ..QueueProperties::default()
        },
        runtime: runtime(active, dead_letter, scheduled),
    }
}

fn topic(name: &str, subscription_count: i64) -> TopicInfo {
    TopicInfo {
        properties: TopicProperties {
            name: name.to_owned(),
            support_ordering: true,
            user_metadata: Some("Demo event stream".to_owned()),
            ..TopicProperties::default()
        },
        subscription_count,
        size_in_bytes: 184_320,
        scheduled_message_count: 1,
        created_at: Some(datetime!(2025-11-03 09:30 UTC)),
        updated_at: Some(datetime!(2026-07-24 16:00 UTC)),
        accessed_at: Some(datetime!(2026-07-24 16:04 UTC)),
    }
}

fn subscription(
    topic: &str,
    name: &str,
    active: i64,
    dead_letter: i64,
    requires_session: bool,
) -> SubscriptionInfo {
    SubscriptionInfo {
        properties: SubscriptionProperties {
            topic: topic.to_owned(),
            name: name.to_owned(),
            requires_session,
            user_metadata: Some("Demo subscription".to_owned()),
            ..SubscriptionProperties::default()
        },
        runtime: runtime(active, dead_letter, 0),
    }
}

fn rule(topic: &str, subscription: &str, name: &str, filter: RuleFilter) -> RuleInfo {
    RuleInfo {
        properties: RuleProperties {
            topic: topic.to_owned(),
            subscription: subscription.to_owned(),
            name: name.to_owned(),
            filter,
            action: None,
        },
        created_at: Some(datetime!(2025-11-03 09:35 UTC)),
    }
}

fn runtime(active: i64, dead_letter: i64, scheduled: i64) -> EntityRuntimeInfo {
    let count_details = MessageCountDetails {
        active,
        dead_letter,
        scheduled,
        ..MessageCountDetails::default()
    };
    EntityRuntimeInfo {
        message_count: count_details.total(),
        size_in_bytes: count_details.total() * 1_536,
        count_details,
        created_at: Some(datetime!(2025-11-03 09:30 UTC)),
        updated_at: Some(datetime!(2026-07-24 16:00 UTC)),
        accessed_at: Some(datetime!(2026-07-24 16:04 UTC)),
    }
}

fn runtime_for(
    messages: &HashMap<MessageSource, Vec<SiftMessage>>,
    path: &EntityPath,
) -> EntityRuntimeInfo {
    let main = messages.get(&source(path.clone(), false));
    let dead_letters = messages.get(&source(path.clone(), true));
    let active = main.map_or(0, |items| {
        count_i64(
            items
                .iter()
                .filter(|message| message.state != MessageState::Scheduled)
                .count(),
        )
    });
    let scheduled = main.map_or(0, |items| {
        count_i64(
            items
                .iter()
                .filter(|message| message.state == MessageState::Scheduled)
                .count(),
        )
    });
    let dead_letter = dead_letters.map_or(0, |items| count_i64(items.len()));
    runtime(active, dead_letter, scheduled)
}

fn source(entity: EntityPath, dead_letter: bool) -> MessageSource {
    MessageSource {
        entity,
        dead_letter,
    }
}

fn seed_source(
    messages: &mut HashMap<MessageSource, Vec<SiftMessage>>,
    path: EntityPath,
    count: usize,
    sequence_offset: i64,
) {
    let rows = (1_i64..)
        .take(count)
        .map(|index| sample_message(sequence_offset + index, path.name(), false))
        .collect();
    messages.insert(source(path, false), rows);
}

fn seed_dead_letters(
    messages: &mut HashMap<MessageSource, Vec<SiftMessage>>,
    path: EntityPath,
    count: usize,
    sequence_offset: i64,
) {
    let rows = (1_i64..)
        .take(count)
        .map(|index| sample_message(sequence_offset + index, path.name(), true))
        .collect();
    messages.insert(source(path, true), rows);
}

fn sample_message(sequence: i64, destination: &str, dead_letter: bool) -> SiftMessage {
    let event = match sequence.rem_euclid(4) {
        0 => "order.created",
        1 => "order.validated",
        2 => "order.allocated",
        _ => "order.completed",
    };
    let order = 1_000 + sequence.rem_euclid(97);
    let amount = 48 + sequence.rem_euclid(17) * 23;
    let body = format!(
        "{{\"eventType\":\"{event}\",\"orderId\":\"ORD-{order}\",\"destination\":\"{destination}\",\"total\":{amount},\"currency\":\"USD\"}}"
    );
    let enqueued =
        datetime!(2026-07-24 16:00 UTC) + time::Duration::seconds(sequence.rem_euclid(3_600));
    let scheduled = !dead_letter && sequence.rem_euclid(13) == 0;

    SiftMessage {
        sequence_number: sequence,
        message_id: Some(format!("msg-{sequence:06}")),
        subject: Some(event.to_owned()),
        content_type: Some("application/json".to_owned()),
        correlation_id: Some(format!("order-{order}")),
        session_id: (destination == "session-work").then(|| "order-1042".to_owned()),
        reply_to: None,
        to: Some(destination.to_owned()),
        enqueued_time: Some(enqueued),
        expires_at: Some(enqueued + time::Duration::days(14)),
        time_to_live: Some(Duration::from_hours(336)),
        delivery_count: Some(if dead_letter { 10 } else { 0 }),
        state: if scheduled {
            MessageState::Scheduled
        } else {
            MessageState::Active
        },
        lock_token: None,
        locked_until: None,
        dead_letter_reason: dead_letter.then(|| "MaxDeliveryCountExceeded".to_owned()),
        dead_letter_error_description: dead_letter
            .then(|| "The simulated handler exhausted its retry policy.".to_owned()),
        dead_letter_source: dead_letter.then(|| destination.to_owned()),
        application_properties: vec![
            ("tenant".to_owned(), "northstar".to_owned()),
            ("region".to_owned(), "westus2".to_owned()),
            ("schemaVersion".to_owned(), "3".to_owned()),
        ],
        body: decode(body.into_bytes()),
    }
}

fn count_i64(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producer_updates_store_and_open_message_view() {
        let mut app = DemoApp::seeded();
        let orders = source(EntityPath::Queue("orders".to_owned()), false);
        let stored_before = app.messages.get(&orders).map_or(0, Vec::len);
        let visible_before = app.message_view_mut(&orders).rows.len();

        app.emit_message();

        assert_eq!(app.generated, 1);
        assert_eq!(
            app.messages.get(&orders).map_or(0, Vec::len),
            stored_before + 1
        );
        assert_eq!(app.message_view_mut(&orders).rows.len(), visible_before + 1);
    }

    #[test]
    fn completing_a_simulated_lock_removes_the_message() {
        let mut app = DemoApp::seeded();
        let orders = source(EntityPath::Queue("orders".to_owned()), false);
        let stored_before = app.messages.get(&orders).map_or(0, Vec::len);

        app.receive(&orders, ReceiveMode::PeekLock, 1);
        let token = app
            .message_view_mut(&orders)
            .rows
            .first()
            .and_then(|message| message.lock_token.clone())
            .expect("peek-lock receive should assign a token");
        app.settle(&orders, &token, Disposition::Complete);

        assert_eq!(
            app.messages.get(&orders).map_or(0, Vec::len),
            stored_before - 1
        );
        assert!(app.message_view_mut(&orders).rows.is_empty());
    }
}
