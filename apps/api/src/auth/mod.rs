pub mod cli;
pub mod delivery;
pub mod login;
mod mail;
pub mod password;
pub(crate) mod store;
pub(crate) mod token;

use url::Url;

#[derive(Clone)]
pub struct AuthConfig {
    pub public_origin: String,
    pub secure_cookies: bool,
    pub session_ttl_seconds: i32,
}

impl AuthConfig {
    pub fn parse(origin: &str, ttl_seconds: i32) -> Result<Self, &'static str> {
        let url = Url::parse(origin).map_err(|_| "must be an HTTP(S) origin")?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("must contain only scheme, host and optional port");
        }
        let loopback = match url.host() {
            Some(url::Host::Domain("localhost")) => true,
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
            return Err("requires HTTPS; HTTP is allowed only on loopback for development");
        }
        if !(60..=86400).contains(&ttl_seconds) {
            return Err("session TTL must be 60..86400 seconds");
        }
        Ok(Self {
            public_origin: url.origin().ascii_serialization(),
            secure_cookies: url.scheme() == "https",
            session_ttl_seconds: ttl_seconds,
        })
    }

    pub fn session_cookie_name(&self) -> &'static str {
        if self.secure_cookies {
            "__Host-turnesta_session"
        } else {
            "turnesta_session"
        }
    }

    pub fn csrf_cookie_name(&self) -> &'static str {
        if self.secure_cookies {
            "__Host-turnesta_csrf"
        } else {
            "turnesta_csrf"
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self::parse("http://127.0.0.1:8080", 28800).expect("valid static development configuration")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_uses_host_only_secure_cookies_and_origin_is_normalized() {
        let config = AuthConfig::parse("https://turnesta.example:443", 3600).unwrap();
        assert!(config.secure_cookies);
        assert_eq!(config.public_origin, "https://turnesta.example");
        assert!(config.session_cookie_name().starts_with("__Host-"));
    }

    #[test]
    fn insecure_non_local_origins_and_urls_with_extra_components_are_rejected() {
        for origin in [
            "http://example.com",
            "http://0.0.0.0:8080",
            "https://user:pass@example.com",
            "https://example.com/path",
            "https://example.com/?x=1",
            "https://example.com/#x",
        ] {
            assert!(AuthConfig::parse(origin, 3600).is_err());
        }
        assert!(AuthConfig::parse("http://localhost:8080", 3600).is_ok());
        assert!(AuthConfig::parse("https://example.com", 0).is_err());
    }
}
