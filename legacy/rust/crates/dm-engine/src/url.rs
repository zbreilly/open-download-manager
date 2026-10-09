//! Minimal URL parser for http/https.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String, // starts with '/'
}

#[derive(Debug, thiserror::Error)]
#[error("bad url: {0}")]
pub struct UrlError(pub String);

impl Url {
    pub fn parse(s: &str) -> Result<Self, UrlError> {
        let (scheme, rest) = s
            .split_once("://")
            .ok_or_else(|| UrlError(s.to_string()))?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(UrlError(s.to_string()));
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        // strip userinfo if present
        let authority = match authority.rfind('@') {
            Some(i) => &authority[i + 1..],
            None => authority,
        };
        let default_port = if scheme == "https" { 443 } else { 80 };
        // bare IPv6 needs bracket handling
        let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
            let (h, p) = rest
                .split_once(']')
                .ok_or_else(|| UrlError(s.to_string()))?;
            let port = p
                .strip_prefix(':')
                .map(|p| p.parse::<u16>())
                .transpose()
                .map_err(|_| UrlError(s.to_string()))?;
            (h.to_string(), port.unwrap_or(default_port))
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => {
                    let port = p.parse::<u16>().map_err(|_| UrlError(s.to_string()))?;
                    (h.to_string(), port)
                }
                None => (authority.to_string(), default_port),
            }
        };
        if host.is_empty() {
            return Err(UrlError(s.to_string()));
        }
        Ok(Self { scheme, host, port, path: path.to_string() })
    }

    pub fn host_port(&self) -> String {
        if (self.scheme == "http" && self.port == 80)
            || (self.scheme == "https" && self.port == 443)
        {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn is_tls(&self) -> bool {
        self.scheme == "https"
    }
}
