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
type ActiveClients = Arc<Mutex<HashMap<IpAddr, usize>>>;

pub(super) struct ClientPermit {
    active: ActiveClients,
    client: IpAddr,
}

impl Drop for ClientPermit {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = active.get_mut(&self.client) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.client);
            }
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct ClientLimit {
    header: Option<HeaderName>,
    windows: Arc<Mutex<ClientWindows>>,
    active: ActiveClients,
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

    fn header_ip(&self, headers: &HeaderMap) -> Option<IpAddr> {
        let value = headers.get(self.header.as_ref()?)?.to_str().ok()?;
        let ip: IpAddr = value.trim().parse().ok()?;
        Some(ip.to_canonical())
    }

    pub(super) fn log_ip(&self, headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
        match &self.header {
            Some(name) if headers.get_all(name).iter().nth(1).is_none() => self.header_ip(headers),
            Some(_) => None,
            None => peer.map(|ip| ip.to_canonical()),
        }
    }

    pub(super) fn client(&self, headers: &HeaderMap) -> Option<IpAddr> {
        match self.header_ip(headers)? {
            IpAddr::V6(ip) => Some(IpAddr::V6((u128::from(ip) >> 64 << 64).into())),
            ip => Some(ip),
        }
    }

    pub(super) fn acquire(&self, headers: &HeaderMap) -> Result<Option<ClientPermit>, ()> {
        if self.header.is_none() {
            return Ok(None);
        }
        let client = self.client(headers).ok_or_else(|| {
            tracing::warn!("configured client IP header is missing or invalid");
        })?;
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        let count = active.entry(client).or_default();
        if *count >= 8 {
            return Err(());
        }
        *count += 1;
        Ok(Some(ClientPermit {
            active: self.active.clone(),
            client,
        }))
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
    fn log_ip_uses_one_trusted_address_and_preserves_the_full_ipv6_address() {
        let peer = Some("192.0.2.19".parse().unwrap());
        let mut proxy = from("2001:db8::17");
        assert_eq!(ClientLimit::default().log_ip(&proxy, peer), peer);
        assert_eq!(
            limit().log_ip(&proxy, peer),
            Some("2001:db8::17".parse().unwrap())
        );
        proxy.append("x-real-ip", "2001:db8::18".parse().unwrap());
        assert_eq!(limit().log_ip(&proxy, peer), None);
        assert_eq!(limit().log_ip(&HeaderMap::new(), peer), None);
        assert_eq!(limit().log_ip(&from("not-an-address"), peer), None);
        assert_eq!(limit().log_ip(&from("::ffff:192.0.2.19"), None), peer);
    }

    #[test]
    fn concurrent_permits_are_scoped_and_released() {
        let limit = limit();
        let first = from("203.0.113.1");
        let other = from("203.0.113.2");
        let mut permits: Vec<_> = (0..8).map(|_| limit.acquire(&first).unwrap()).collect();
        assert!(limit.acquire(&first).is_err());
        assert!(limit.acquire(&other).is_ok());
        permits.pop();
        assert!(limit.acquire(&first).is_ok());
        drop(permits);
        assert!(limit.active.lock().unwrap().is_empty());
        assert!(ClientLimit::default().acquire(&HeaderMap::new()).is_ok());
        assert!(limit.acquire(&HeaderMap::new()).is_err());
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
