//! FTP and FTPS downloads: a small client for what a download needs
//! (log in, binary mode, `SIZE`, `MDTM`, `REST`, passive `RETR`).
//!
//! * `ftp://`   plain FTP
//! * `ftpes://` explicit TLS (`AUTH TLS` on the normal port, 21)
//! * `ftps://`  implicit TLS (TLS from the first byte, port 990)
//!
//! Passive mode only (`EPSV`, then `PASV`). The data connection always goes
//! to the address of the control connection, never to the one in the
//! server's `PASV` answer. Every segment of a download opens its own
//! control connection and starts its transfer with `REST`, so the task
//! code splits and resumes FTP downloads the same way as HTTP ones.
//! The proxy setting does not apply to FTP.

use crate::model::{Category, ProbeResult, Settings};
use crate::util;
use crate::CoreError;
use bytes::Bytes;
use futures_util::stream::BoxStream;
use percent_encoding::percent_decode_str;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub fn is_ftp(url: &str) -> bool {
    let u = url.trim();
    let scheme = u.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("ftp" | "ftps" | "ftpes"))
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tls {
    None,
    /// `AUTH TLS` after the greeting
    Explicit,
    /// TLS from the start
    Implicit,
}

/// Where a file is and how to log in.
#[derive(Clone)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// the path sent to the server (relative to the login folder unless the
    /// URL starts it with `%2F`)
    pub path: String,
    pub user: String,
    pub pass: String,
    pub tls: Tls,
    pub timeout: Duration,
    tls_config: Option<Arc<rustls::ClientConfig>>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint").field("host", &self.host).field("port", &self.port).field("tls", &self.tls).finish()
    }
}

/// What went wrong with an FTP command.
#[derive(Debug)]
pub enum FtpError {
    /// connection problem or timeout: trying again may help
    Io(String),
    /// the server answered with this reply
    Reply(u16, String),
    /// TLS setup failed
    Tls(String),
    /// the file's size on the server is not the one expected
    Changed { expected: u64, now: u64 },
    /// the server can't continue a transfer from an offset
    NoResume,
}

impl FtpError {
    /// Worth trying again later (network trouble, server busy).
    pub fn retryable(&self) -> bool {
        match self {
            FtpError::Io(_) => true,
            // 421 service not available / too many connections, 425/426
            // data connection trouble, 450/451 temporary file errors
            FtpError::Reply(c, _) => matches!(c, 421 | 425 | 426 | 450 | 451),
            _ => false,
        }
    }

    /// The server limits connections (fewer connections may work).
    pub fn throttled(&self) -> bool {
        matches!(self, FtpError::Reply(421, _))
    }

    pub fn message(&self) -> String {
        match self {
            FtpError::Io(m) => m.clone(),
            FtpError::Tls(m) => format!("FTPS: {m}"),
            FtpError::Reply(530, _) => {
                "FTP login failed: check the user name and password (Add dialog → Authentication)".into()
            }
            FtpError::Reply(c @ (550 | 553), t) => format!("FTP {c}: file not found or access denied ({})", clip(t)),
            FtpError::Reply(421, t) => format!("FTP 421: the server refused the connection ({})", clip(t)),
            FtpError::Reply(c, t) => format!("FTP {c}: {}", clip(t)),
            FtpError::Changed { expected, now } => format!("file changed on server (size {expected} -> {now}); restarting"),
            FtpError::NoResume => "the FTP server can't resume transfers".into(),
        }
    }
}

fn clip(t: &str) -> String {
    let t = t.trim();
    let mut s: String = t.chars().take(120).collect();
    if s.len() < t.len() {
        s.push('…');
    }
    util::redact_urls(&s)
}

fn io_err(e: std::io::Error) -> FtpError {
    FtpError::Io(match e.kind() {
        std::io::ErrorKind::ConnectionRefused => "could not connect to server".into(),
        std::io::ErrorKind::TimedOut => "connection timed out".into(),
        std::io::ErrorKind::UnexpectedEof => "the server closed the connection".into(),
        _ => format!("connection error: {e}"),
    })
}

impl Endpoint {
    /// `user`/`pass` override the ones in the URL; without either, the
    /// login is anonymous.
    pub fn new(url: &str, user: Option<&str>, pass: Option<&str>, s: &Settings) -> Result<Self, CoreError> {
        let u = url::Url::parse(url.trim()).map_err(|_| CoreError::BadUrl(util::redact_url(url)))?;
        let tls = match u.scheme() {
            "ftp" => Tls::None,
            "ftpes" => Tls::Explicit,
            "ftps" => Tls::Implicit,
            other => return Err(CoreError::BadUrl(format!("unsupported scheme: {other}"))),
        };
        let host = u.host_str().filter(|h| !h.is_empty()).ok_or_else(|| CoreError::BadUrl(util::redact_url(url)))?;
        let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
        let port = u.port().unwrap_or(if tls == Tls::Implicit { 990 } else { 21 });
        let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
        // RFC 1738: the path is relative to the login folder; "%2F" at the
        // start makes it absolute. `;type=i` is dropped.
        let raw = u.path().split(';').next().unwrap_or("");
        let raw = raw.strip_prefix('/').unwrap_or(raw);
        let path = decode(raw);
        if path.is_empty() || path.ends_with('/') {
            return Err(CoreError::BadUrl("the FTP link names a folder, not a file".into()));
        }
        let (url_user, url_pass) = (decode(u.username()), u.password().map(decode));
        let user = user.filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(url_user);
        let pass = pass.filter(|s| !s.is_empty()).map(str::to_string).or(url_pass).unwrap_or_default();
        let (user, pass) = if user.is_empty() { ("anonymous".to_string(), "anonymous@".to_string()) } else { (user, pass) };
        let tls_config = if tls == Tls::None { None } else { Some(tls_config(s.verify_tls)?) };
        Ok(Self { host, port, path, user, pass, tls, timeout: Duration::from_secs(s.timeout_secs.max(5)), tls_config })
    }

    fn server_name(&self) -> Result<rustls::pki_types::ServerName<'static>, FtpError> {
        rustls::pki_types::ServerName::try_from(self.host.clone()).map_err(|_| FtpError::Tls("invalid host name".into()))
    }

    async fn tls_wrap<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(&self, s: S) -> Result<Box<dyn Io>, FtpError> {
        let cfg = self.tls_config.clone().ok_or_else(|| FtpError::Tls("not configured".into()))?;
        let c = tokio_rustls::TlsConnector::from(cfg);
        let name = self.server_name()?;
        let t = tokio::time::timeout(self.timeout, c.connect(name, s))
            .await
            .map_err(|_| FtpError::Io("connection timed out".into()))?
            .map_err(|e| FtpError::Tls(e.to_string()))?;
        Ok(Box::new(t))
    }

    async fn tcp(&self, addr: SocketAddr) -> Result<TcpStream, FtpError> {
        let s = tokio::time::timeout(self.timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| FtpError::Io("connection timed out".into()))?
            .map_err(io_err)?;
        let _ = s.set_nodelay(true);
        Ok(s)
    }

    /// Connect and log in, in binary mode.
    async fn login(&self) -> Result<Control, FtpError> {
        let addrs = tokio::time::timeout(self.timeout, tokio::net::lookup_host((self.host.as_str(), self.port)))
            .await
            .map_err(|_| FtpError::Io("connection timed out".into()))?
            .map_err(|_| FtpError::Io("server name could not be resolved".into()))?;
        let mut last = FtpError::Io("server name could not be resolved".into());
        let mut tcp = None;
        for a in addrs {
            match self.tcp(a).await {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e) => last = e,
            }
        }
        let tcp = tcp.ok_or(last)?;
        let peer = tcp.peer_addr().map_err(io_err)?.ip();
        let mut ctl = match self.tls {
            Tls::Implicit => {
                let s = self.tls_wrap(tcp).await?;
                Control::new(s, peer, self.timeout)
            }
            Tls::None => Control::new(Box::new(tcp), peer, self.timeout),
            Tls::Explicit => {
                let mut plain = Control::new(Box::new(tcp), peer, self.timeout);
                plain.expect_greeting().await?;
                let r = plain.cmd("AUTH TLS").await?;
                if r.0 != 234 {
                    return Err(FtpError::Tls(format!("the server does not offer TLS ({} {})", r.0, clip(&r.1))));
                }
                // the server sends nothing more until the handshake, so the
                // reader holds no buffered bytes
                let inner = plain.rd.into_inner();
                let s = self.tls_wrap(inner).await?;
                let mut c = Control::new(s, peer, self.timeout);
                c.greeted = true;
                c
            }
        };
        if !ctl.greeted {
            ctl.expect_greeting().await?;
        }
        if self.tls != Tls::None {
            ctl.cmd("PBSZ 0").await?;
            let r = ctl.cmd("PROT P").await?;
            if r.0 != 200 {
                return Err(FtpError::Tls(format!("the server refused encrypted transfers ({} {})", r.0, clip(&r.1))));
            }
            ctl.secure_data = true;
        }
        let r = ctl.cmd(&format!("USER {}", self.user)).await?;
        match r.0 {
            230 => {}
            331 | 332 => {
                let r = ctl.cmd(&format!("PASS {}", self.pass)).await?;
                if !matches!(r.0, 230 | 202) {
                    return Err(FtpError::Reply(r.0, r.1));
                }
            }
            c => return Err(FtpError::Reply(c, r.1)),
        }
        ctl.ok("TYPE I", &[200]).await?;
        Ok(ctl)
    }

    /// Size, modification time and resume support of the file.
    pub async fn stat(&self) -> Result<FileStat, FtpError> {
        let mut ctl = self.login().await?;
        let size = ctl.size(&self.path).await?;
        let r = ctl.cmd(&format!("MDTM {}", self.path)).await?;
        let modified = (r.0 == 213).then(|| r.1.trim().to_string()).filter(|m| !m.is_empty());
        if size.is_none() && modified.is_none() && r.0 == 550 {
            return Err(FtpError::Reply(550, r.1));
        }
        let rest = ctl.cmd("REST 0").await?;
        let _ = ctl.cmd("QUIT").await;
        Ok(FileStat { size, modified, resumable: rest.0 == 350 })
    }

    /// Start a transfer at byte `from`. `expect_total` is the size the
    /// download was started with; a different size on the server means the
    /// file changed.
    pub async fn open(&self, from: u64, expect_total: Option<u64>) -> Result<BoxStream<'static, Result<Bytes, String>>, FtpError> {
        let mut ctl = self.login().await?;
        if let Some(t) = expect_total {
            if let Some(now) = ctl.size(&self.path).await? {
                if now != t {
                    return Err(FtpError::Changed { expected: t, now });
                }
            }
        }
        if from > 0 {
            let r = ctl.cmd(&format!("REST {from}")).await?;
            if r.0 != 350 {
                return Err(FtpError::NoResume);
            }
        }
        let addr = ctl.passive().await?;
        let tcp = self.tcp(addr).await?;
        ctl.send(&format!("RETR {}", self.path)).await?;
        let (reply, data) = if ctl.secure_data {
            let (r, d) = tokio::join!(ctl.reply(), self.tls_wrap(tcp));
            (r?, d)
        } else {
            (ctl.reply().await?, Ok(Box::new(tcp) as Box<dyn Io>))
        };
        if !matches!(reply.0, 125 | 150) {
            return Err(FtpError::Reply(reply.0, reply.1));
        }
        let data = data?;
        Ok(data_stream(data, ctl))
    }
}

/// File facts from the server.
#[derive(Debug, Clone)]
pub struct FileStat {
    pub size: Option<u64>,
    /// `MDTM` answer (YYYYMMDDHHMMSS)
    pub modified: Option<String>,
    pub resumable: bool,
}

/// The body of a `RETR`: data until the server closes the data
/// connection, then the transfer must be confirmed on the control
/// connection (226/250), or the stream ends with an error.
fn data_stream(data: Box<dyn Io>, ctl: Control) -> BoxStream<'static, Result<Bytes, String>> {
    struct St {
        data: Option<Box<dyn Io>>,
        ctl: Control,
        buf: Vec<u8>,
    }
    let st = St { data: Some(data), ctl, buf: vec![0u8; 64 * 1024] };
    Box::pin(futures_util::stream::unfold(Some(st), |st| async move {
        let mut st = st?;
        let timeout = st.ctl.timeout;
        let data = st.data.as_mut()?;
        match tokio::time::timeout(timeout, data.read(&mut st.buf)).await {
            Err(_) => Some((Err("connection timed out".to_string()), None)),
            Ok(Err(e)) => Some((Err(io_err(e).message()), None)),
            Ok(Ok(0)) => {
                // closing our side ends the transfer for TLS servers too
                if let Some(mut d) = st.data.take() {
                    let _ = d.shutdown().await;
                }
                match st.ctl.reply().await {
                    Ok((226 | 250, _)) => {
                        let _ = st.ctl.send("QUIT").await;
                        None
                    }
                    Ok((c, t)) => Some((Err(FtpError::Reply(c, t).message()), None)),
                    Err(e) => Some((Err(e.message()), None)),
                }
            }
            Ok(Ok(n)) => {
                let chunk = Bytes::copy_from_slice(&st.buf[..n]);
                Some((Ok(chunk), Some(st)))
            }
        }
    }))
}

struct Control {
    rd: BufReader<Box<dyn Io>>,
    peer: IpAddr,
    timeout: Duration,
    greeted: bool,
    secure_data: bool,
}

impl Control {
    fn new(s: Box<dyn Io>, peer: IpAddr, timeout: Duration) -> Self {
        Self { rd: BufReader::new(s), peer, timeout, greeted: false, secure_data: false }
    }

    async fn expect_greeting(&mut self) -> Result<(), FtpError> {
        let r = self.reply().await?;
        self.greeted = true;
        if r.0 != 220 {
            return Err(FtpError::Reply(r.0, r.1));
        }
        Ok(())
    }

    async fn send(&mut self, line: &str) -> Result<(), FtpError> {
        if line.contains(['\r', '\n']) {
            return Err(FtpError::Io("invalid characters in the FTP command".into()));
        }
        let w = self.rd.get_mut();
        let fut = async {
            w.write_all(line.as_bytes()).await?;
            w.write_all(b"\r\n").await?;
            w.flush().await
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| FtpError::Io("connection timed out".into()))?
            .map_err(io_err)
    }

    /// One reply (multi-line replies end with "NNN ").
    async fn reply(&mut self) -> Result<(u16, String), FtpError> {
        let timeout = self.timeout;
        let rd = &mut self.rd;
        let fut = async {
            let mut text = String::new();
            let mut code: Option<u16> = None;
            loop {
                let mut line = Vec::new();
                let n = rd.read_until(b'\n', &mut line).await.map_err(io_err)?;
                if n == 0 {
                    return Err(FtpError::Io("the server closed the connection".into()));
                }
                if line.len() > 8192 {
                    return Err(FtpError::Io("the server sent an invalid reply".into()));
                }
                let l = String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string();
                let num = l.get(..3).and_then(|c| c.parse::<u16>().ok());
                let sep = l.as_bytes().get(3).copied();
                match code {
                    None => {
                        let Some(c) = num else {
                            return Err(FtpError::Io("the server sent an invalid reply".into()));
                        };
                        text.push_str(l.get(4..).unwrap_or(""));
                        if sep != Some(b'-') {
                            return Ok((c, text));
                        }
                        code = Some(c);
                    }
                    Some(c) => {
                        if num == Some(c) && sep == Some(b' ') {
                            text.push('\n');
                            text.push_str(l.get(4..).unwrap_or(""));
                            return Ok((c, text));
                        }
                        text.push('\n');
                        text.push_str(&l);
                    }
                }
            }
        };
        tokio::time::timeout(timeout, fut).await.map_err(|_| FtpError::Io("connection timed out".into()))?
    }

    async fn cmd(&mut self, line: &str) -> Result<(u16, String), FtpError> {
        self.send(line).await?;
        self.reply().await
    }

    async fn ok(&mut self, line: &str, want: &[u16]) -> Result<(u16, String), FtpError> {
        let r = self.cmd(line).await?;
        if want.contains(&r.0) {
            Ok(r)
        } else {
            Err(FtpError::Reply(r.0, r.1))
        }
    }

    /// `SIZE`: None when the server doesn't support it.
    async fn size(&mut self, path: &str) -> Result<Option<u64>, FtpError> {
        let r = self.cmd(&format!("SIZE {path}")).await?;
        Ok(match r.0 {
            213 => r.1.trim().parse().ok(),
            _ => None,
        })
    }

    /// Enter passive mode; the address to connect the data connection to.
    async fn passive(&mut self) -> Result<SocketAddr, FtpError> {
        let r = self.cmd("EPSV").await?;
        if r.0 == 229 {
            if let Some(p) = parse_epsv(&r.1) {
                return Ok(SocketAddr::new(self.peer, p));
            }
        }
        let r = self.cmd("PASV").await?;
        if r.0 != 227 {
            return Err(FtpError::Reply(r.0, r.1));
        }
        // the port only: the host part of the answer is ignored (it can be
        // a private address behind NAT, or point at another machine)
        let p = parse_pasv(&r.1).ok_or_else(|| FtpError::Io("the server sent an invalid passive-mode reply".into()))?;
        Ok(SocketAddr::new(self.peer, p))
    }
}

/// Port from "Entering Extended Passive Mode (|||6446|)".
pub fn parse_epsv(text: &str) -> Option<u16> {
    let inner = text.split_once('(')?.1.split_once(')')?.0;
    let d = inner.chars().next()?;
    let parts: Vec<&str> = inner.split(d).collect();
    parts.get(3)?.trim().parse().ok().filter(|p| *p > 0)
}

/// Port from "Entering Passive Mode (h1,h2,h3,h4,p1,p2)".
pub fn parse_pasv(text: &str) -> Option<u16> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let nums: Vec<u16> = text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(6)
        .map(|s| s.parse().ok())
        .collect::<Option<Vec<u16>>>()?;
    if nums.len() < 6 || nums[4] > 255 || nums[5] > 255 {
        return None;
    }
    Some(nums[4] * 256 + nums[5]).filter(|p| *p > 0)
}

fn tls_config(verify: bool) -> Result<Arc<rustls::ClientConfig>, CoreError> {
    crate::client::install_crypto();
    let cfg = if verify {
        use rustls_platform_verifier::ConfigVerifierExt;
        rustls::ClientConfig::with_platform_verifier().map_err(|e| CoreError::Other(format!("TLS: {e}")))?
    } else {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny))
            .with_no_client_auth()
    };
    Ok(Arc::new(cfg))
}

/// Accepts any certificate (Settings → Advanced → "Verify TLS" off).
#[derive(Debug)]
struct AcceptAny;

impl rustls::client::danger::ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

/// A URL with any user name and password removed (they live in the
/// download's credential fields, which are stored sealed).
pub fn split_credentials(url: &str) -> (String, Option<String>, Option<String>) {
    match url::Url::parse(url.trim()) {
        Ok(mut u) if is_ftp(url) => {
            let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
            let user = Some(decode(u.username())).filter(|s| !s.is_empty());
            let pass = u.password().map(decode).filter(|s| !s.is_empty());
            let _ = u.set_username("");
            let _ = u.set_password(None);
            (u.to_string(), user, pass)
        }
        _ => (url.trim().to_string(), None, None),
    }
}

/// Probe an FTP link: size, modification time, resume support.
pub async fn probe(ep: &Endpoint, url: &str, cats: &[Category]) -> Result<ProbeResult, CoreError> {
    let st = ep.stat().await.map_err(|e| CoreError::Network(e.message()))?;
    let (clean, _, _) = split_credentials(url);
    let name = util::filename_from_url(&clean).unwrap_or_else(|| "download".into());
    Ok(ProbeResult {
        category: util::category_for(&name, cats),
        final_url: clean,
        file_name: name,
        total_bytes: st.size,
        mime: None,
        resumable: st.resumable && st.size.is_some(),
        protocol: match ep.tls {
            Tls::None => "FTP".into(),
            Tls::Explicit => "FTPS (explicit TLS)".into(),
            Tls::Implicit => "FTPS (implicit TLS)".into(),
        },
        etag: None,
        last_modified: st.modified,
        stream: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_replies() {
        assert_eq!(parse_epsv("Entering Extended Passive Mode (|||6446|)"), Some(6446));
        assert_eq!(parse_epsv("Entering Extended Passive Mode (!!!21!)"), Some(21));
        assert_eq!(parse_epsv("nope"), None);
        assert_eq!(parse_pasv("Entering Passive Mode (192,168,1,2,19,137)."), Some(19 * 256 + 137));
        assert_eq!(parse_pasv("=10,0,0,1,4,1"), Some(1025));
        assert_eq!(parse_pasv("(1,2,3)"), None);
    }

    #[test]
    fn endpoints() {
        let s = Settings::default();
        let e = Endpoint::new("ftp://u%40x:p%3Aw@files.example/pub/a%20b.zip;type=i", None, None, &s).unwrap();
        assert_eq!((e.host.as_str(), e.port, e.path.as_str()), ("files.example", 21, "pub/a b.zip"));
        assert_eq!((e.user.as_str(), e.pass.as_str()), ("u@x", "p:w"));
        let e = Endpoint::new("ftps://h.example/%2Fabs/f.iso", Some("me"), Some("pw"), &s).unwrap();
        assert_eq!((e.port, e.path.as_str(), e.user.as_str(), e.tls), (990, "/abs/f.iso", "me", Tls::Implicit));
        let e = Endpoint::new("ftpes://h.example:2121/f", None, None, &s).unwrap();
        assert_eq!((e.port, e.user.as_str(), e.tls), (2121, "anonymous", Tls::Explicit));
        assert!(Endpoint::new("ftp://h.example/dir/", None, None, &s).is_err());
        assert!(is_ftp("FTP://x/y") && is_ftp("ftps://x/y") && !is_ftp("https://x/y"));
        let (u, user, pass) = split_credentials("ftp://me:secret@h.example/f.zip");
        assert_eq!((u.as_str(), user.as_deref(), pass.as_deref()), ("ftp://h.example/f.zip", Some("me"), Some("secret")));
    }
}
