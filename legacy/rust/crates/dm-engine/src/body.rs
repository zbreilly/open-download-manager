//! Response body readers: Content-Length, chunked, or read-to-EOF.

use crate::http::body_kind::BodyKind;
use crate::http::Connection;
use std::io::Read;

pub struct BodyReader {
    conn: Connection,
    kind: BodyKind,
    remaining: u64, // ContentLength mode
    chunk_left: usize,
    chunk_done: bool,
    eof: bool,
}

impl BodyReader {
    pub fn new(conn: Connection, kind: BodyKind) -> Self {
        let remaining = match kind {
            BodyKind::ContentLength(n) => n,
            _ => 0,
        };
        Self {
            conn,
            kind,
            remaining,
            chunk_left: 0,
            chunk_done: false,
            eof: false,
        }
    }
}

impl Read for BodyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.eof || buf.is_empty() {
            return Ok(0);
        }
        match self.kind {
            BodyKind::ContentLength(_) => {
                if self.remaining == 0 {
                    self.eof = true;
                    return Ok(0);
                }
                let want = buf.len().min(self.remaining as usize);
                let n = self.conn.read(&mut buf[..want])?;
                if n == 0 {
                    // server lied about length — treat as end
                    self.eof = true;
                    return Ok(0);
                }
                self.remaining -= n as u64;
                Ok(n)
            }
            BodyKind::Chunked => self.read_chunked(buf),
            BodyKind::ToEof => {
                let n = self.conn.read(buf)?;
                if n == 0 {
                    self.eof = true;
                }
                Ok(n)
            }
        }
    }
}

impl BodyReader {
    fn read_chunked(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.chunk_done {
                self.eof = true;
                return Ok(0);
            }
            if self.chunk_left > 0 {
                let want = buf.len().min(self.chunk_left);
                let n = self.conn.read(&mut buf[..want])?;
                if n == 0 {
                    self.eof = true;
                    return Ok(0);
                }
                self.chunk_left -= n;
                if self.chunk_left == 0 {
                    self.consume_crlf()?;
                }
                return Ok(n);
            }
            // read next chunk size line
            let line = self.read_line()?;
            let size_str = line.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_str, 16).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "bad chunk size")
            })?;
            if size == 0 {
                // trailers until blank line
                loop {
                    let t = self.read_line()?;
                    if t.is_empty() {
                        break;
                    }
                }
                self.chunk_done = true;
                return Ok(0);
            }
            self.chunk_left = size;
        }
    }

    fn read_line(&mut self) -> std::io::Result<String> {
        let mut line = Vec::with_capacity(16);
        let mut byte = [0u8; 1];
        loop {
            let n = self.conn.read(&mut byte)?;
            if n == 0 {
                break;
            }
            if byte[0] == b'\n' {
                break;
            }
            if byte[0] != b'\r' {
                line.push(byte[0]);
            }
        }
        Ok(String::from_utf8_lossy(&line).into_owned())
    }

    fn consume_crlf(&mut self) -> std::io::Result<()> {
        let mut seen = 0;
        while seen < 2 {
            let mut b = [0u8; 1];
            let n = self.conn.read(&mut b)?;
            if n == 0 {
                break;
            }
            if b[0] == b'\n' || b[0] == b'\r' {
                seen += 1;
            }
        }
        Ok(())
    }
}
