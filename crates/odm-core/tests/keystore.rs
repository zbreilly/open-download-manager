//! Secrets sealed with the key the system's keystore holds (macOS and
//! Linux): what is in the database, and what happens when the keystore does
//! not answer. One test, in a process of its own: the key is process-wide.
#![cfg(not(windows))]

use odm_core::secret::set_keystore_for_tests as keystore;
use odm_core::*;
use std::path::Path;
use std::time::Duration;

const SECRETS: [&str; 4] = ["PROXY-SECRET-1", "COOKIE-SECRET-2", "PASSWORD-SECRET-3", "HEADER-SECRET-4"];

fn database_bytes(dir: &Path) -> Vec<u8> {
    let mut all = vec![];
    for name in ["odm.db", "odm.db-wal"] {
        all.extend(std::fs::read(dir.join(name)).unwrap_or_default());
    }
    all
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

fn open(dir: &Path) -> Manager {
    Manager::open(&dir.join("odm.db")).unwrap()
}

async fn close(m: Manager) {
    // writes every download back, as leaving the app does
    m.shutdown_wait(Duration::from_secs(5)).await;
    drop(m);
}

fn add_with_secrets(m: &Manager) -> String {
    let mut s = m.settings();
    s.proxy_pass = SECRETS[0].into();
    s.proxy_mode = "none".into();
    m.save_settings(s).unwrap();
    m.add(AddRequest {
        url: "http://127.0.0.1:9/f/a.bin".into(),
        cookies: Some(format!("session={}", SECRETS[1])),
        username: Some("user".into()),
        password: Some(SECRETS[2].into()),
        headers: vec![("X-Api-Key".into(), SECRETS[3].into())],
        ..Default::default()
    })
    .unwrap()
}

fn assert_secrets_readable(m: &Manager, id: &str) {
    let d = m.get(id).unwrap();
    assert_eq!(d.cookies, Some(format!("session={}", SECRETS[1])));
    assert_eq!(d.password.as_deref(), Some(SECRETS[2]));
    assert_eq!(d.headers, vec![("X-Api-Key".to_string(), SECRETS[3].to_string())]);
    assert_eq!(m.settings().proxy_pass, SECRETS[0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sealed_with_the_keystore_key() {
    let key = [7u8; 32];
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();

    // --- with the key: nothing readable in the database
    keystore(Some(key), false);
    let m = open(dir);
    let id = add_with_secrets(&m);
    let pairing = m.bridge_key();
    close(m).await;
    let raw = database_bytes(dir);
    assert!(contains(&raw, "a.bin") && contains(&raw, "odm-key1:"), "the database was read");
    for secret in SECRETS.iter().copied().chain([pairing.as_str()]) {
        assert!(!contains(&raw, secret), "{secret} is readable in the database");
    }

    // --- the keystore does not answer: nothing is read, nothing is lost
    keystore(None, true);
    let m = open(dir);
    let d = m.get(&id).unwrap();
    assert_eq!((d.cookies, d.password), (None, None));
    assert_eq!(d.headers, vec![("X-Api-Key".to_string(), String::new())], "header names are kept, values unread");
    assert_eq!(m.settings().proxy_pass, "");
    assert_ne!(m.bridge_key(), pairing, "a key for this run only");
    // the app goes on writing: settings, this download, everything at exit
    let mut s = m.settings();
    s.max_retries = 7;
    m.save_settings(s).unwrap();
    m.update_download(&id, manager::DownloadPatch { description: Some("edited while locked".into()), ..Default::default() });
    close(m).await;
    for secret in SECRETS.iter().copied().chain([pairing.as_str()]) {
        assert!(!contains(&database_bytes(dir), secret), "{secret} leaked while locked");
    }

    // --- the keystore answers again: everything is back
    keystore(Some(key), false);
    let m = open(dir);
    assert_secrets_readable(&m, &id);
    assert_eq!(m.get(&id).unwrap().description.as_deref(), Some("edited while locked"));
    assert_eq!(m.settings().max_retries, 7);
    assert_eq!(m.bridge_key(), pairing, "the pairing key survives a run without the keystore");
    close(m).await;

    // --- another key (the keystore's item was replaced): the old secrets
    // are gone, not garbage
    keystore(Some([9u8; 32]), false);
    let m = open(dir);
    let d = m.get(&id).unwrap();
    assert_eq!((d.cookies, d.password), (None, None));
    assert_ne!(m.bridge_key(), pairing);
    close(m).await;

    // --- secrets stored as they are (no keystore then) are sealed the
    // first time the keystore answers
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    keystore(None, false);
    let m = open(dir);
    let id = add_with_secrets(&m);
    let pairing = m.bridge_key();
    close(m).await;
    assert!(contains(&database_bytes(dir), SECRETS[1]), "stored as it is without a keystore");
    keystore(Some(key), false);
    let m = open(dir);
    assert_secrets_readable(&m, &id);
    assert_eq!(m.bridge_key(), pairing);
    close(m).await;
    let raw = database_bytes(dir);
    for secret in SECRETS.iter().copied().chain([pairing.as_str()]) {
        assert!(!contains(&raw, secret), "{secret} was not sealed once the keystore answered");
    }
}
