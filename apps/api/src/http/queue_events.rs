use super::{
    AppState,
    api::{ApiError, SignedIn, check_origin},
};
use crate::{
    auth::store,
    core::{TenantId, UserId},
    queue,
    queue_live::{self, LiveHub, QueueKey, Signal},
};
use axum::{
    Router,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::get,
};
use futures_util::stream;
use serde::Deserialize;
use std::{
    convert::Infallible,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, watch};
use uuid::Uuid;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/events",
            get(staff),
        )
        .route("/api/public/queues/{slug}/events", get(public))
        .route(
            "/assets/queue-live.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/src/queueLive.mjs"),
                )
            }),
        )
        .route(
            "/assets/queue-live.css",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/src/queueLive.css"),
                )
            }),
        )
}

fn hub(state: &AppState) -> Result<Arc<LiveHub>, ApiError> {
    state
        .queue_live
        .as_ref()
        .filter(|hub| hub.running())
        .cloned()
        .ok_or_else(ApiError::unavailable)
}
fn slot(hub: &LiveHub, public: bool) -> Result<OwnedSemaphorePermit, ApiError> {
    hub.acquire(public).ok_or_else(|| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_streams",
            "Live connection capacity reached",
        )
    })
}
fn origin(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    if headers.contains_key("origin") {
        check_origin(headers, &state.auth)?;
    }
    Ok(())
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Audience {
    #[default]
    Guest,
    Display,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PublicQuery {
    #[serde(default)]
    audience: Audience,
}

#[utoipa::path(get,path="/api/organizations/{tenant_id}/queues/{queue_id}/events",tag="queue",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)),responses((status=200,description="SSE refresh/heartbeat; snapshots remain authoritative",content_type="text/event-stream"),(status=429,description="Live connection capacity reached")))]
pub(super) async fn staff(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
) -> Result<Response, ApiError> {
    origin(&headers, &state)?;
    let Path((tenant, queue_id)) = path.map_err(|_| ApiError::bad_request())?;
    let hub = hub(&state)?;
    let permit = slot(&hub, false)?;
    tokio::time::timeout(
        queue_live::QUERY_TIMEOUT,
        queue::get(&state.pool, signed.user_id, tenant, queue_id),
    )
    .await
    .map_err(|_| ApiError::unavailable())??;
    let key = QueueKey {
        tenant,
        queue: queue_id,
    };
    Ok(connection(
        state,
        hub.subscribe(key),
        permit,
        Access::Staff {
            user: signed.user_id,
            hash: signed.token_hash,
            key,
        },
    ))
}

#[utoipa::path(get,path="/api/public/queues/{slug}/events",tag="queue-public",params(("slug"=Uuid,Path),("audience"=Option<String>,Query,description="guest (default) or display")),responses((status=200,description="SSE generic refresh hints, no ticket data",content_type="text/event-stream"),(status=404,description="Requested public audience is not published"),(status=429,description="Live connection capacity reached")))]
pub(super) async fn public(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<PublicQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    origin(&headers, &state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    let hub = hub(&state)?;
    let permit = slot(&hub, true)?;
    let display = matches!(query.audience, Audience::Display);
    let target = tokio::time::timeout(
        queue_live::QUERY_TIMEOUT,
        queue_live::public_target(&state.pool, slug, display),
    )
    .await
    .map_err(|_| ApiError::unavailable())??
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Public queue not available",
        )
    })?;
    Ok(connection(
        state,
        hub.subscribe(target.key()),
        permit,
        Access::Public { slug, display },
    ))
}

enum Access {
    Staff {
        user: UserId,
        hash: [u8; 32],
        key: QueueKey,
    },
    Public {
        slug: Uuid,
        display: bool,
    },
}
struct Connection {
    state: AppState,
    receiver: watch::Receiver<Signal>,
    _permit: OwnedSemaphorePermit,
    access: Access,
    first: bool,
    closed: bool,
    revision: Option<String>,
    authorized_at: Instant,
}
impl Connection {
    fn end(&mut self, name: &str) -> Event {
        self.closed = true;
        Event::default().event(name).data("{}")
    }
    async fn authorize_due(&mut self) -> Option<Event> {
        let Access::Staff { user, hash, key } = &self.access else {
            return None;
        };
        if self.authorized_at.elapsed() < Duration::from_secs(15) {
            return None;
        }
        let verify = async {
            let session = store::find_session(&self.state.pool, hash)
                .await
                .map_err(ApiError::from)?;
            if !session.is_some_and(|session| session.user_id == *user) {
                return Ok(false);
            }
            match queue::get(&self.state.pool, *user, key.tenant, key.queue).await {
                Ok(_) => Ok(true),
                Err(queue::QueueError::Core(
                    crate::core::CoreError::AccessDenied
                    | crate::core::CoreError::ReferenceNotFound,
                )) => Ok(false),
                Err(error) => Err(ApiError::from(error)),
            }
        };
        match tokio::time::timeout(queue_live::QUERY_TIMEOUT, verify).await {
            Ok(Ok(true)) => {
                self.authorized_at = Instant::now();
                None
            }
            Ok(Ok(false)) => Some(self.end("denied")),
            _ => Some(self.end("unavailable")),
        }
    }
    async fn next(&mut self) -> Option<Event> {
        if self.closed {
            return None;
        }
        loop {
            if let Some(event) = self.authorize_due().await {
                return Some(event);
            }
            let signal = if self.first {
                self.first = false;
                self.receiver.borrow_and_update().clone()
            } else {
                let auth_wait =
                    Duration::from_secs(15).saturating_sub(self.authorized_at.elapsed());
                let change = tokio::select! {
                    _=tokio::time::sleep(auth_wait), if matches!(self.access,Access::Staff{..}) => continue,
                    value=tokio::time::timeout(Duration::from_secs(10), self.receiver.changed())=>value,
                };
                match change {
                    Ok(Ok(())) => self.receiver.borrow_and_update().clone(),
                    _ => return Some(self.end("unavailable")),
                }
            };
            match signal {
                Signal::Pending => continue,
                Signal::Unavailable | Signal::Stopping => return Some(self.end("unavailable")),
                Signal::Ready(row, checked) => {
                    if let Some(event) = self.authorize_due().await {
                        return Some(event);
                    }
                    if checked.elapsed() > Duration::from_secs(10) {
                        return Some(self.end("unavailable"));
                    }
                    match &self.access {
                        Access::Public { slug, display }
                            if row.public_slug != Some(*slug)
                                || (*display && !row.display_enabled) =>
                        {
                            return Some(self.end("denied"));
                        }
                        _ => {}
                    }
                    let revision = row.event_id();
                    let name = if self.revision.as_ref() == Some(&revision) {
                        "heartbeat"
                    } else {
                        "refresh"
                    };
                    self.revision = Some(revision.clone());
                    return Some(
                        Event::default()
                            .event(name)
                            .id(revision)
                            .retry(Duration::from_secs(2))
                            .data("{}"),
                    );
                }
            }
        }
    }
}
fn connection(
    state: AppState,
    receiver: watch::Receiver<Signal>,
    permit: OwnedSemaphorePermit,
    access: Access,
) -> Response {
    let connection = Connection {
        state,
        receiver,
        _permit: permit,
        access,
        first: true,
        closed: false,
        revision: None,
        authorized_at: Instant::now(),
    };
    let events = stream::unfold(connection, |mut connection| async move {
        connection
            .next()
            .await
            .map(|event| (Ok::<_, Infallible>(event), connection))
    });
    let mut response = Sse::new(events).into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().expect("static header"));
    response
}
