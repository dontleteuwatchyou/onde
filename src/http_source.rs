//! Reads a remote file as if it were local, one ranged request at a time.

use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

use symphonia::core::io::MediaSource;

const CHUNK_BYTES: u64 = 512 * 1024;
/// For servers that ignore ranges and send the whole file at once.
const WHOLE_FILE_LIMIT: u64 = 1024 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
const ATTEMPTS: usize = 2;

pub struct HttpSource {
    agent: ureq::Agent,
    url: String,
    position: u64,
    total: u64,
    buffer: Vec<u8>,
    buffer_start: u64,
}

impl HttpSource {
    pub fn open(url: String) -> io::Result<Self> {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(TIMEOUT))
                .http_status_as_error(false)
                .build(),
        );
        let mut source = Self {
            agent,
            url,
            position: 0,
            total: 0,
            buffer: Vec::new(),
            buffer_start: 0,
        };
        source.fetch(0)?;
        Ok(source)
    }

    fn fetch(&mut self, start: u64) -> io::Result<()> {
        let mut result = self.fetch_once(start);
        for _ in 1..ATTEMPTS {
            if result.is_ok() {
                break;
            }
            result = self.fetch_once(start);
        }
        result
    }

    /// Errors never quote the URL: it carries the credentials.
    fn fetch_once(&mut self, start: u64) -> io::Result<()> {
        let range = format!("bytes={start}-{}", start + CHUNK_BYTES - 1);
        let mut response = self
            .agent
            .get(&self.url)
            .header("Range", &range)
            .call()
            .map_err(|_| io::Error::other("serveur injoignable"))?;
        let status = response.status().as_u16();
        let read_error = |_| io::Error::other("téléchargement interrompu");
        match status {
            206 => {
                let total = response
                    .headers()
                    .get("content-range")
                    .and_then(|value| value.to_str().ok())
                    .and_then(content_range_total);
                self.buffer = response
                    .body_mut()
                    .with_config()
                    .limit(CHUNK_BYTES + 1)
                    .read_to_vec()
                    .map_err(read_error)?;
                self.buffer_start = start;
                self.total = total.unwrap_or(start + self.buffer.len() as u64);
            }
            200 => {
                self.buffer = response
                    .body_mut()
                    .with_config()
                    .limit(WHOLE_FILE_LIMIT)
                    .read_to_vec()
                    .map_err(read_error)?;
                self.buffer_start = 0;
                self.total = self.buffer.len() as u64;
            }
            other => return Err(io::Error::other(format!("le serveur a répondu {other}"))),
        }
        Ok(())
    }
}

/// `bytes 0-99/1234` gives 1234. A total of `*` means unknown.
fn content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.trim().parse().ok()
}

impl Read for HttpSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.total || out.is_empty() {
            return Ok(0);
        }
        let end = self.buffer_start + self.buffer.len() as u64;
        if self.position < self.buffer_start || self.position >= end {
            self.fetch(self.position)?;
        }
        let Some(offset) = self.position.checked_sub(self.buffer_start) else {
            return Ok(0);
        };
        let available = self.buffer.get(offset as usize..).unwrap_or_default();
        let count = available.len().min(out.len());
        out[..count].copy_from_slice(&available[..count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(offset) => self.total.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        };
        self.position = target
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "position invalide"))?;
        Ok(self.position)
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.total)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    /// Serves `body` on localhost, honouring ranges when `ranges` is set.
    pub fn serve(body: Vec<u8>, ranges: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut range = None;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let not_found = first.contains("/missing");
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_lowercase().strip_prefix("range: bytes=") {
                        let (start, end) = value.trim().split_once('-').unwrap();
                        range = Some((
                            start.parse::<usize>().unwrap(),
                            end.parse::<usize>().unwrap(),
                        ));
                    }
                }
                let reply = if not_found {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                } else {
                    match range.filter(|_| ranges) {
                        Some((start, end)) => {
                            let end = end.min(body.len() - 1);
                            let part = &body[start..=end];
                            let mut out = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                                part.len(),
                                body.len()
                            )
                            .into_bytes();
                            out.extend(part);
                            out
                        }
                        None => {
                            let mut out = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            out.extend(&body);
                            out
                        }
                    }
                };
                let _ = stream.write_all(&reply);
            }
        });
        format!("http://{address}")
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn reads_a_file_larger_than_a_chunk() {
        let body = sample(CHUNK_BYTES as usize * 2 + 100);
        let mut source = HttpSource::open(serve(body.clone(), true)).unwrap();
        assert_eq!(source.byte_len(), Some(body.len() as u64));
        let mut out = Vec::new();
        source.read_to_end(&mut out).unwrap();
        assert!(out == body);
    }

    #[test]
    fn seeks_forward_and_back() {
        let body = sample(CHUNK_BYTES as usize * 3);
        let mut source = HttpSource::open(serve(body.clone(), true)).unwrap();
        let target = CHUNK_BYTES as usize * 2 + 7;
        source.seek(SeekFrom::Start(target as u64)).unwrap();
        let mut out = [0; 16];
        source.read_exact(&mut out).unwrap();
        assert_eq!(out, body[target..target + 16]);

        source.seek(SeekFrom::Start(3)).unwrap();
        source.read_exact(&mut out).unwrap();
        assert_eq!(out, body[3..19]);

        source.seek(SeekFrom::End(-4)).unwrap();
        let mut end = Vec::new();
        source.read_to_end(&mut end).unwrap();
        assert_eq!(end, body[body.len() - 4..]);
    }

    #[test]
    fn works_with_a_server_that_ignores_ranges() {
        let body = sample(CHUNK_BYTES as usize + 50);
        let mut source = HttpSource::open(serve(body.clone(), false)).unwrap();
        source.seek(SeekFrom::Start(CHUNK_BYTES)).unwrap();
        let mut out = Vec::new();
        source.read_to_end(&mut out).unwrap();
        assert_eq!(out, body[CHUNK_BYTES as usize..]);
    }

    #[test]
    fn errors_do_not_reveal_the_url() {
        let url = format!("{}/missing?t=SECRET", serve(sample(10), true));
        let error = HttpSource::open(url).err().unwrap().to_string();
        assert!(error.contains("404"), "{error}");
        assert!(!error.contains("SECRET"), "{error}");

        let refused = HttpSource::open("http://127.0.0.1:1/?t=SECRET".into());
        assert!(!refused.err().unwrap().to_string().contains("SECRET"));
    }
}
