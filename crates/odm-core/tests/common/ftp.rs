//! Minimal FTP server for tests: passive mode (EPSV/PASV), SIZE, MDTM,
//! REST, RETR, with failure modes.
//!
//! Files are `/name` → bytes. Log in with the configured user and password
//! (`anonymous` is refused unless allowed).
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Default)]
pub struct FtpOpts {
    pub user: String,
    pub pass: String,
    pub allow_anonymous: bool,
    /// close the data connection after this many bytes ...
    pub drop_after: Option<u64>,
    /// ... for this many transfers
    pub drops: u64,
    /// pause after every 16 KiB sent (ms)
    pub delay_ms: u64,
    /// answer 421 when this many sessions are open
    pub max_sessions: Option<usize>,
    /// no EPSV (forces PASV)
    pub no_epsv: bool,
    /// no REST (resume not supported)
    pub no_rest: bool,
}

#[derive(Clone)]
pub struct FtpServer {
    pub port: u16,
    pub files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    /// commands received (PASS hidden), in order
    pub log: Arc<Mutex<Vec<String>>>,
    pub sessions: Arc<AtomicUsize>,
    pub transfers: Arc<AtomicU64>,
    opts: Arc<FtpOpts>,
}

impl FtpServer {
    pub fn url(&self, path: &str) -> String {
        format!("ftp://127.0.0.1:{}{path}", self.port)
    }

    pub fn put(&self, name: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(name.to_string(), data);
    }

    pub fn commands(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// Offsets of every REST command.
    pub fn rests(&self) -> Vec<u64> {
        self.commands().iter().filter_map(|c| c.strip_prefix("REST ")).filter_map(|v| v.parse().ok()).collect()
    }
}

pub async fn start(opts: FtpOpts) -> FtpServer {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let srv = FtpServer {
        port: l.local_addr().unwrap().port(),
        files: Arc::new(Mutex::new(HashMap::new())),
        log: Arc::new(Mutex::new(vec![])),
        sessions: Arc::new(AtomicUsize::new(0)),
        transfers: Arc::new(AtomicU64::new(0)),
        opts: Arc::new(opts),
    };
    let s2 = srv.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { continue };
            let srv = s2.clone();
            tokio::spawn(async move {
                let n = srv.sessions.fetch_add(1, Ordering::SeqCst);
                session(s, &srv, n).await;
                srv.sessions.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    srv
}

async fn reply(w: &mut tokio::net::tcp::OwnedWriteHalf, t: String) -> std::io::Result<()> {
    w.write_all((t + "\r\n").as_bytes()).await
}

async fn session(s: TcpStream, srv: &FtpServer, open_before: usize) {
    let (r, mut w) = s.into_split();
    let mut r = BufReader::new(r);
    let o = srv.opts.clone();
    if o.max_sessions.is_some_and(|m| open_before >= m) {
        let _ = w.write_all(b"421 Too many connections from your address\r\n").await;
        return;
    }
    let _ = w.write_all(b"220-Test FTP server\r\n220 ready\r\n").await;
    let mut user = String::new();
    let mut logged_in = false;
    let mut rest: u64 = 0;
    let mut pasv: Option<TcpListener> = None;
    loop {
        let mut line = String::new();
        match r.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = line.trim_end().to_string();
        let (cmd, arg) = line.split_once(' ').map(|(c, a)| (c.to_ascii_uppercase(), a.to_string())).unwrap_or((line.to_ascii_uppercase(), String::new()));
        srv.log.lock().unwrap().push(if cmd == "PASS" { "PASS ***".into() } else { line.clone() });
        let ok = match cmd.as_str() {
            "USER" => {
                user = arg.clone();
                reply(&mut w, "331 Password required".into()).await
            }
            "PASS" => {
                let good = (user == o.user && arg == o.pass) || (o.allow_anonymous && user == "anonymous");
                logged_in = good;
                reply(&mut w, if good { "230 Logged in".into() } else { "530 Login incorrect".into() }).await
            }
            "QUIT" => {
                let _ = reply(&mut w, "221 Bye".into()).await;
                return;
            }
            "AUTH" => reply(&mut w, "502 TLS not available".into()).await,
            _ if !logged_in => reply(&mut w, "530 Please log in".into()).await,
            "TYPE" => reply(&mut w, "200 Type set".into()).await,
            "SIZE" => {
                let f = srv.files.lock().unwrap().get(&format!("/{}", arg.trim_start_matches('/'))).map(|f| f.len());
                reply(&mut w, f.map(|n| format!("213 {n}")).unwrap_or_else(|| "550 No such file".into())).await
            }
            "MDTM" => {
                let f = srv.files.lock().unwrap().contains_key(&format!("/{}", arg.trim_start_matches('/')));
                reply(&mut w, if f { "213 20260101120000".into() } else { "550 No such file".into() }).await
            }
            "REST" if o.no_rest => reply(&mut w, "502 REST not implemented".into()).await,
            "REST" => {
                rest = arg.parse().unwrap_or(0);
                reply(&mut w, format!("350 Restarting at {rest}")).await
            }
            "EPSV" if o.no_epsv => reply(&mut w, "502 EPSV not implemented".into()).await,
            "EPSV" | "PASV" => {
                let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let p = l.local_addr().unwrap().port();
                pasv = Some(l);
                if cmd == "EPSV" {
                    reply(&mut w, format!("229 Entering Extended Passive Mode (|||{p}|)")).await
                } else {
                    // a bogus host: the client must use the control address
                    reply(&mut w, format!("227 Entering Passive Mode (10,9,8,7,{},{})", p / 256, p % 256)).await
                }
            }
            "RETR" => {
                let data = srv.files.lock().unwrap().get(&format!("/{}", arg.trim_start_matches('/'))).cloned();
                let Some(data) = data else {
                    let _ = reply(&mut w, "550 No such file".into()).await;
                    continue;
                };
                let Some(l) = pasv.take() else {
                    let _ = reply(&mut w, "425 Use PASV first".into()).await;
                    continue;
                };
                let _ = reply(&mut w, "150 Opening BINARY mode data connection".into()).await;
                let Ok((mut d, _)) = l.accept().await else { return };
                let n = srv.transfers.fetch_add(1, Ordering::SeqCst);
                let start = (rest as usize).min(data.len());
                rest = 0;
                let drop_at = o.drop_after.filter(|_| n < o.drops);
                let mut sent = 0u64;
                let mut complete = true;
                for chunk in data[start..].chunks(16 * 1024) {
                    let chunk = match drop_at {
                        Some(at) if sent + chunk.len() as u64 > at => &chunk[..(at - sent) as usize],
                        _ => chunk,
                    };
                    if d.write_all(chunk).await.is_err() {
                        complete = false;
                        break;
                    }
                    sent += chunk.len() as u64;
                    if drop_at.is_some_and(|at| sent >= at) {
                        complete = false;
                        break;
                    }
                    if o.delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(o.delay_ms)).await;
                    }
                }
                drop(d);
                if complete {
                    reply(&mut w, "226 Transfer complete".into()).await
                } else {
                    // an aborted transfer: drop the control connection too
                    return;
                }
            }
            _ => reply(&mut w, "502 Command not implemented".into()).await,
        };
        if ok.is_err() {
            return;
        }
    }
}
