use axum::http::{HeaderMap, HeaderName};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(3600);
const LIMIT: u32 = 10;
const MAX_TRACKED: usize = 10_000;

#[derive(Clone, Default)]
pub(super) struct ClientLimit {
    header: Option<HeaderName>,
    windows: Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>,
}

impl ClientLimit {
    pub(super) fn new(header: Option<HeaderName>) -> Self {
        Self {
            header,
            ..Self::default()
        }
    }

    pub(super) fn allows(&self, headers: &HeaderMap) -> bool {
        self.client(headers)
            .is_none_or(|client| self.record(client, Instant::now()))
    }

    fn client(&self, headers: &HeaderMap) -> Option<IpAddr> {
        let value = headers.get(self.header.as_ref()?)?.to_str().ok()?;
        match value.trim().parse().ok()? {
            IpAddr::V6(ip) => Some(IpAddr::V6((u128::from(ip) >> 64 << 64).into())),
            ip => Some(ip),
        }
    }

    fn record(&self, client: IpAddr, now: Instant) -> bool {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if windows.len() >= MAX_TRACKED {
            windows.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
            if windows.len() >= MAX_TRACKED {
                windows.clear();
            }
        }
        let entry = windows.entry(client).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= LIMIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit() -> ClientLimit {
        ClientLimit::new(Some(HeaderName::from_static("x-real-ip")))
    }
    fn from(ip: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", ip.parse().unwrap());
        headers
    }

    #[test]
    fn counts_each_client_separately_within_the_window() {
        let limit = limit();
        for _ in 0..LIMIT {
            assert!(limit.allows(&from("203.0.113.1")));
        }
        assert!(!limit.allows(&from("203.0.113.1")));
        assert!(limit.allows(&from("203.0.113.2")));
    }

    #[test]
    fn window_expires() {
        let limit = limit();
        let ip = "203.0.113.1".parse().unwrap();
        let start = Instant::now();
        for _ in 0..=LIMIT {
            limit.record(ip, start);
        }
        assert!(!limit.record(ip, start + WINDOW - Duration::from_secs(1)));
        assert!(limit.record(ip, start + WINDOW));
    }

    #[test]
    fn ipv6_addresses_of_one_network_share_a_limit() {
        let limit = limit();
        for host in 0..LIMIT {
            assert!(limit.allows(&from(&format!("2001:db8:1:1::{host:x}"))));
        }
        assert!(!limit.allows(&from("2001:db8:1:1::ffff")));
        assert!(limit.allows(&from("2001:db8:1:2::1")));
    }

    #[test]
    fn without_a_configured_header_or_address_nothing_is_limited() {
        let direct = ClientLimit::default();
        for _ in 0..=LIMIT {
            assert!(direct.allows(&from("203.0.113.1")));
        }
        let limit = limit();
        for _ in 0..=LIMIT {
            assert!(limit.allows(&HeaderMap::new()));
            assert!(limit.allows(&from("not-an-address")));
        }
    }
}
