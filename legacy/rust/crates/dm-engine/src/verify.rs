//! Post-download integrity: SHA-256 verification of the assembled file.

use crate::EngineError;
use sha2::{Digest, Sha256};
use std::path::Path;

fn file_sha256(path: &Path) -> Result<[u8; 32], EngineError> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

fn hex_ok(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// Download (single- or multi-segment), then verify SHA-256.
/// On mismatch the error is `EngineError::ChecksumMismatch` and the
/// `.part` file is deliberately left in place for diagnosis.
pub fn download_verified(
    url: &str,
    out: &Path,
    parts: usize,
    expected_sha256: &str,
) -> Result<crate::DownloadStats, EngineError> {
    let expected = hex_ok(expected_sha256)
        .ok_or_else(|| EngineError::BadUrl("sha256 must be 64 hex chars".into()))?;
    let stats = if parts >= 2 {
        crate::multi::download_multi(url, out, parts, None, None)?
    } else {
        crate::download(url, out)?
    };
    let got = file_sha256(out)?;
    if got != expected {
        return Err(EngineError::ChecksumMismatch {
            expected: hex(&expected),
            got: hex(&got),
        });
    }
    Ok(stats)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
