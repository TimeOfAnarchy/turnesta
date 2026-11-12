use axum::http::{HeaderMap, HeaderName};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(3600);
const LIMIT: u32 = 10;
const DAY: Duration = Duration::from_secs(86400);
const DAY_LIMIT: u32 = 30;
const MAX_TRACKED: usize = 10_000;

type ClientWindows = HashMap<IpAddr, (Instant, u32, Instant, u32)>;

#[derive(Clone, Default)]
pub(super) struct ClientLimit {
    header: Option<HeaderName>,
    windows: Arc<Mutex<ClientWindows>>,
}

impl ClientLimit {
    pub(super) fn new(header: Option<HeaderName>) -> Self {
        Self {
            header,
            ..Self::default()
        }
    }

    pub(super) fn allows(&self, headers: &HeaderMap) -> bool {
        if self.header.is_none() {
            return true;
        }
        match self.client(headers) {
            Some(client) => self.record(client, Instant::now()),
            None => {
                tracing::warn!("configured client IP header is missing or invalid");
                false
            }
        }
    }

    pub(super) fn client(&self, headers: &HeaderMap) -> Option<IpAddr> {
        let value = headers.get(self.header.as_ref()?)?.to_str().ok()?;
        let ip: IpAddr = value.trim().parse().ok()?;
        match ip.to_canonical() {
            IpAddr::V6(ip) => Some(IpAddr::V6((u128::from(ip) >> 64 << 64).into())),
            ip => Some(ip),
        }
    }

    fn record(&self, client: IpAddr, now: Instant) -> bool {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if !windows.contains_key(&client) && windows.len() >= MAX_TRACKED {
            windows.retain(|_, (_, _, day_start, _)| now.duration_since(*day_start) < DAY);
            if windows.len() >= MAX_TRACKED
                && let Some(oldest) = windows
                    .iter()
                    .min_by_key(|(_, entry)| entry.2)
                    .map(|(ip, _)| *ip)
            {
                windows.remove(&oldest);
            }
        }
        let entry = windows.entry(client).or_insert((now, 0, now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            entry.0 = now;
            entry.1 = 0;
        }
        if now.duration_since(entry.2) >= DAY {
            entry.2 = now;
            entry.3 = 0;
        }
        entry.1 = entry.1.saturating_add(1);
        entry.3 = entry.3.saturating_add(1);
        entry.1 <= LIMIT && entry.3 <= DAY_LIMIT
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
    fn daily_limit_survives_hourly_resets() {
        let limit = limit();
        let ip = "203.0.113.1".parse().unwrap();
        let start = Instant::now();
        for hour in 0..3 {
            for _ in 0..10 {
                assert!(limit.record(ip, start + WINDOW * hour));
            }
        }
        assert!(!limit.record(ip, start + WINDOW * 3));
        assert!(limit.record(ip, start + DAY));
    }

    #[test]
    fn capacity_evicts_oldest_instead_of_clearing_all() {
        let limit = limit();
        let start = Instant::now();
        for n in 0..MAX_TRACKED {
            let ip = IpAddr::V4((n as u32).into());
            assert!(limit.record(ip, start + Duration::from_millis(n as u64)));
        }
        let new = IpAddr::V4((MAX_TRACKED as u32).into());
        assert!(limit.record(new, start + Duration::from_secs(11)));
        let windows = limit.windows.lock().unwrap();
        assert_eq!(windows.len(), MAX_TRACKED);
        assert!(!windows.contains_key(&IpAddr::V4(0_u32.into())));
        assert!(windows.contains_key(&IpAddr::V4(1_u32.into())));
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
    fn mapped_ipv4_addresses_share_the_ipv4_bucket() {
        let limit = limit();
        assert_eq!(
            limit.client(&from("::ffff:203.0.113.1")),
            limit.client(&from("203.0.113.1"))
        );
    }

    #[test]
    fn configured_header_fails_closed_when_address_is_missing_or_invalid() {
        let direct = ClientLimit::default();
        for _ in 0..=LIMIT {
            assert!(direct.allows(&from("203.0.113.1")));
        }
        let limit = limit();
        for _ in 0..=LIMIT {
            assert!(!limit.allows(&HeaderMap::new()));
            assert!(!limit.allows(&from("not-an-address")));
        }
    }
}
