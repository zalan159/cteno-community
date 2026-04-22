//! Notification watcher polling loop.
//!
//! Polls the macOS notification center database every 5 seconds,
//! delivering new notifications to subscribed Persona chat sessions.

use std::path::PathBuf;
use std::time::Duration;

use super::macos_db;
use super::store::NotificationStore;

/// The main notification watcher.
pub struct NotificationWatcher {
    store: NotificationStore,
}

impl NotificationWatcher {
    pub fn new(db_path: PathBuf) -> Self {
        let store = NotificationStore::new(db_path);
        Self { store }
    }

    /// Access the underlying store (for RPC handlers).
    pub fn store(&self) -> &NotificationStore {
        &self.store
    }

    /// Main loop: polls every 5 seconds for new notifications.
    pub async fn run(&self) {
        log::info!("[NotifWatcher] Starting notification watcher...");

        // Check if macOS notification DB is accessible
        let db_path = macos_db::db_path();
        if !db_path.exists() {
            log::warn!(
                "[NotifWatcher] macOS notification DB not found at {}. Watcher disabled.",
                db_path.display()
            );
            return;
        }

        // Initialize watermarks for all currently subscribed apps
        self.initialize_watermarks();

        log::info!("[NotifWatcher] Entering poll loop (5s interval)");
        loop {
            if let Err(e) = self.poll_once().await {
                log::error!("[NotifWatcher] Poll error: {}", e);
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// For newly subscribed apps that have no watermark yet,
    /// set the watermark to the current max rec_id to avoid replaying history.
    fn initialize_watermarks(&self) {
        let apps = match self.store.active_app_identifiers() {
            Ok(a) => a,
            Err(e) => {
                log::warn!("[NotifWatcher] Failed to list active apps: {}", e);
                return;
            }
        };

        for app in &apps {
            match self.store.get_watermark(app) {
                Ok(0) => {
                    // No watermark yet — set to current max
                    match macos_db::get_max_rec_id(app) {
                        Ok(max_id) => {
                            if let Err(e) = self.store.set_watermark(app, max_id) {
                                log::warn!(
                                    "[NotifWatcher] Failed to init watermark for {}: {}",
                                    app,
                                    e
                                );
                            } else {
                                log::info!(
                                    "[NotifWatcher] Initialized watermark for {} at rec_id={}",
                                    app,
                                    max_id
                                );
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "[NotifWatcher] Failed to get max rec_id for {}: {}",
                                app,
                                e
                            );
                        }
                    }
                }
                Ok(_) => {} // Already has a watermark
                Err(e) => {
                    log::warn!("[NotifWatcher] Failed to get watermark for {}: {}", app, e);
                }
            }
        }
    }

    /// Single poll iteration: check all subscribed apps for new notifications.
    async fn poll_once(&self) -> Result<(), String> {
        let apps = self.store.active_app_identifiers()?;

        for app in &apps {
            let watermark = self.store.get_watermark(app)?;

            // If watermark is 0, this is a newly added subscription —
            // initialize to current max to avoid flooding.
            if watermark == 0 {
                let max_id = macos_db::get_max_rec_id(app).unwrap_or(0);
                if max_id > 0 {
                    self.store.set_watermark(app, max_id)?;
                    log::info!(
                        "[NotifWatcher] Late-initialized watermark for {} at rec_id={}",
                        app,
                        max_id
                    );
                }
                continue;
            }

            let records = match macos_db::fetch_new_records(app, watermark) {
                Ok(r) => r,
                Err(e) => {
                    log::debug!("[NotifWatcher] Failed to fetch records for {}: {}", app, e);
                    continue;
                }
            };

            if records.is_empty() {
                continue;
            }

            let subs = self.store.subscriptions_for_app(app)?;
            let mut max_rec_id = watermark;

            for record in &records {
                if record.rec_id > max_rec_id {
                    max_rec_id = record.rec_id;
                }
                for sub in &subs {
                    self.deliver_to_persona(sub, record).await;
                }
            }

            // Update watermark
            if max_rec_id > watermark {
                self.store.set_watermark(app, max_rec_id)?;
            }
        }

        Ok(())
    }

    /// Deliver a notification to a persona's chat session.
    ///
    /// The runtime does not know about PersonaManager or SpawnSessionConfig;
    /// the host installs a `NotificationDeliveryProvider` impl (see
    /// `hooks::install_notification_delivery`) that knows how to route the
    /// formatted message.  If no provider is installed (e.g. headless test
    /// harness), delivery is a no-op.
    async fn deliver_to_persona(
        &self,
        sub: &super::models::NotificationSubscription,
        record: &super::models::NotificationRecord,
    ) {
        // Skip empty notifications
        if record.body.is_empty() && record.title.is_empty() {
            return;
        }

        let provider = match crate::hooks::notification_delivery() {
            Some(p) => p,
            None => {
                log::debug!("[NotifWatcher] NotificationDeliveryProvider not installed; skipping");
                return;
            }
        };

        provider
            .deliver_to_persona(
                &sub.persona_id,
                &sub.app_display_name,
                &record.title,
                &record.body,
            )
            .await;
    }
}
