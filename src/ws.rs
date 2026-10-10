//! Just enough WebSocket (RFC 6455) to talk to the Rex server over its unix
//! socket: client handshake, masked sends, unmasked reads, ping/pong. No
//! extensions are offered, so the server never compresses.
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const TEXT: u8 = 1;
pub const BINARY: u8 = 2;
const CLOSE: u8 = 8;
const PING: u8 = 9;
const PONG: u8 = 10;

/// The sending half. Cloned freely; every frame goes out whole under the lock.
#[derive(Clone)]
pub struct Writer(Arc<Mutex<UnixStream>>);

pub struct Reader {
    stream: BufReader<UnixStream>,
    writer: Writer,
}

pub fn connect(socket: &Path, resource: &str, protocol: &str) -> io::Result<(Writer, Reader)> {
    let mut stream = UnixStream::connect(socket)?;
    let key = crate::base64::encode(&random16());
    let request = format!(
        "GET {resource} HTTP/1.1\r\nHost: rex\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: {protocol}\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    if !status.contains(" 101 ") {
        return Err(io::Error::other(format!("rex refused the {resource} upgrade: {}", status.trim())));
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if line == "\r\n" {
            break;
        }
    }
    let writer = Writer(Arc::new(Mutex::new(stream)));
    Ok((writer.clone(), Reader { stream: reader, writer }))
}

impl Writer {
    pub fn send(&self, op: u8, payload: &[u8]) -> io::Result<()> {
        let mask = random16();
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x80 | op);
        match payload.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n < 65536 => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask[..4]);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.0.lock().unwrap().write_all(&frame)
    }

    pub fn shutdown(&self) {
        let _ = self.0.lock().unwrap().shutdown(std::net::Shutdown::Both);
    }
}

impl Reader {
    /// The next whole data message as (opcode, payload). Answers pings on the
    /// way; a close frame or a dropped socket is an error.
    pub fn recv(&mut self) -> io::Result<(u8, Vec<u8>)> {
        let mut message: Option<(u8, Vec<u8>)> = None;
        loop {
            let mut head = [0u8; 2];
            self.stream.read_exact(&mut head)?;
            let (fin, op) = (head[0] & 0x80 != 0, head[0] & 0x0f);
            let len = match head[1] & 0x7f {
                126 => {
                    let mut b = [0u8; 2];
                    self.stream.read_exact(&mut b)?;
                    u16::from_be_bytes(b) as usize
                }
                127 => {
                    let mut b = [0u8; 8];
                    self.stream.read_exact(&mut b)?;
                    u64::from_be_bytes(b) as usize
                }
                n => n as usize,
            };
            let mut payload = vec![0u8; len];
            self.stream.read_exact(&mut payload)?;
            match op {
                PING => self.writer.send(PONG, &payload)?,
                PONG => {}
                CLOSE => return Err(io::ErrorKind::ConnectionAborted.into()),
                0 => message.as_mut().ok_or_else(|| io::Error::other("stray continuation frame"))?.1.extend(payload),
                _ => message = Some((op, payload)),
            }
            if fin && op < 8 {
                if let Some(m) = message.take() {
                    return Ok(m);
                }
            }
        }
    }
}

fn random16() -> [u8; 16] {
    let mut b = [0u8; 16];
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    b
}
