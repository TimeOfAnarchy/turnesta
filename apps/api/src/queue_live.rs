use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::core::TenantId;
use sha2::{Digest, Sha256};
use sqlx::{
    PgPool,
    postgres::{PgListener, PgPoolOptions},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};
use uuid::Uuid;

pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueueKey {
    pub tenant: TenantId,
    pub queue: Uuid,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LiveState {
    pub tenant_id: TenantId,
    pub queue_id: Uuid,
    pub revision: String,
    pub public_slug: Option<Uuid>,
    pub display_enabled: bool,
}
impl LiveState {
    pub fn key(&self) -> QueueKey {
        QueueKey {
            tenant: self.tenant_id,
            queue: self.queue_id,
        }
    }
    pub fn event_id(&self) -> String {
        hex::encode(Sha256::digest(self.revision.as_bytes()))
    }
}

#[derive(Debug, Clone)]
pub enum Signal {
    Pending,
    Ready(LiveState, Instant),
    Unavailable,
    Stopping,
}

pub struct LiveHub {
    pool: PgPool,
    channels: Mutex<HashMap<QueueKey, watch::Sender<Signal>>>,
    wake: Notify,
    stop: watch::Sender<bool>,
    running: AtomicBool,
    listener_connected: AtomicBool,
    public_slots: Arc<Semaphore>,
    staff_slots: Arc<Semaphore>,
}
impl LiveHub {
    pub fn new(pool: PgPool) -> Arc<Self> {
        Arc::new(Self {
            pool,
            channels: Mutex::new(HashMap::new()),
            wake: Notify::new(),
            stop: watch::channel(false).0,
            running: AtomicBool::new(false),
            listener_connected: AtomicBool::new(false),
            public_slots: Arc::new(Semaphore::new(96)),
            staff_slots: Arc::new(Semaphore::new(32)),
        })
    }
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire) && !*self.stop.borrow()
    }
    pub fn listener_connected(&self) -> bool {
        self.listener_connected.load(Ordering::Acquire)
    }
    pub fn queue_count(&self) -> usize {
        self.channels.lock().expect("live registry poisoned").len()
    }
    pub fn acquire(&self, public: bool) -> Option<OwnedSemaphorePermit> {
        if !self.running() {
            return None;
        }
        (if public {
            &self.public_slots
        } else {
            &self.staff_slots
        })
        .clone()
        .try_acquire_owned()
        .ok()
    }
    pub fn subscribe(&self, key: QueueKey) -> watch::Receiver<Signal> {
        let mut channels = self.channels.lock().expect("live registry poisoned");
        let receiver = channels
            .entry(key)
            .or_insert_with(|| watch::channel(Signal::Pending).0)
            .subscribe();
        self.wake.notify_one();
        receiver
    }
    pub fn shutdown(&self) {
        self.stop.send_replace(true);
        for sender in self
            .channels
            .lock()
            .expect("live registry poisoned")
            .values()
        {
            sender.send_replace(Signal::Stopping);
        }
    }
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        assert!(
            !self.running.swap(true, Ordering::AcqRel),
            "live hub can only start once"
        );
        let hub = self.clone();
        tokio::spawn(async move {
            let listener_hub = hub.clone();
            let listener = tokio::spawn(async move {
                listener_hub.listen().await;
            });
            let mut stop = hub.stop.subscribe();
            let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if *stop.borrow() {
                    break;
                }
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = interval.tick() => {},
                    _ = hub.wake.notified() => {
                        tokio::select! { _=stop.changed()=>break, _=tokio::time::sleep(Duration::from_millis(40))=>{} }
                    },
                }
                tokio::select! { _ = stop.changed()=>break, _ = hub.reconcile()=>{} }
            }
            hub.shutdown();
            // Dropping a PgListener returns its one dedicated connection before
            // the primary application pool is closed.
            let mut listener = listener;
            if tokio::time::timeout(Duration::from_secs(4), &mut listener)
                .await
                .is_err()
            {
                listener.abort();
                let _ = listener.await;
            }
            hub.running.store(false, Ordering::Release);
        })
    }
    pub async fn reconcile(&self) {
        let targets: Vec<_> = {
            let mut channels = self.channels.lock().expect("live registry poisoned");
            channels.retain(|_, sender| sender.receiver_count() > 0);
            channels.keys().copied().collect()
        };
        if targets.is_empty() {
            return;
        }
        let tenants: Vec<_> = targets.iter().map(|key| key.tenant).collect();
        let queues: Vec<_> = targets.iter().map(|key| key.queue).collect();
        let result = tokio::time::timeout(
            QUERY_TIMEOUT,
            sqlx::query_as::<_, LiveState>("SELECT * FROM queue.live_states($1,$2)")
                .bind(&tenants)
                .bind(&queues)
                .fetch_all(&self.pool),
        )
        .await;
        let states: HashMap<_, _> = match result {
            Ok(Ok(rows)) => rows.into_iter().map(|row| (row.key(), row)).collect(),
            _ => HashMap::new(),
        };
        let checked = Instant::now();
        let channels = self.channels.lock().expect("live registry poisoned");
        for key in targets {
            if let Some(sender) = channels.get(&key) {
                sender.send_replace(
                    states
                        .get(&key)
                        .map(|row| Signal::Ready(row.clone(), checked))
                        .unwrap_or(Signal::Unavailable),
                );
            }
        }
    }
    async fn listen(&self) {
        let listener_pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(QUERY_TIMEOUT)
            .connect_lazy_with(self.pool.connect_options().as_ref().clone());
        let mut stop = self.stop.subscribe();
        while !*stop.borrow() {
            let connect = async {
                let mut listener = PgListener::connect_with(&listener_pool).await?;
                listener.listen("turnesta_queue").await?;
                Ok::<_, sqlx::Error>(listener)
            };
            let connected = tokio::select! { _=stop.changed()=>break, value=tokio::time::timeout(QUERY_TIMEOUT,connect)=>value };
            if let Ok(Ok(mut listener)) = connected {
                self.listener_connected.store(true, Ordering::Release);
                self.wake.notify_one();
                loop {
                    let value = tokio::select! { _=stop.changed()=>break, value=listener.try_recv()=>value };
                    match value {
                        Ok(Some(_)) => self.wake.notify_one(),
                        // Rebuild after disconnect and reconcile after LISTEN;
                        // missed notifications are repaired from row versions.
                        Ok(None) | Err(_) => break,
                    }
                }
            }
            self.listener_connected.store(false, Ordering::Release);
            tokio::select! { _=stop.changed()=>break, _=tokio::time::sleep(Duration::from_secs(1))=>{} }
        }
        self.listener_connected.store(false, Ordering::Release);
        listener_pool.close().await;
    }
}

pub async fn public_target(
    pool: &PgPool,
    slug: Uuid,
    display: bool,
) -> Result<Option<LiveState>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM queue.public_stream_target($1,$2)")
        .bind(slug)
        .bind(display)
        .fetch_optional(pool)
        .await
}
