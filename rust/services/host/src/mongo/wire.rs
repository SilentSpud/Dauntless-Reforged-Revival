//! Just enough of MongoDB's wire protocol for the replica-set check: `OP_MSG` (MongoDB 3.6 and newer)
//! with one body section, and the BSON types the commands send and `hello` answers. The Node kit
//! borrowed the backend's driver for this; three admin commands do not need one.

use std::io::{Read, Write};
use std::net::TcpStream;

const OP_MSG: i32 = 2013;
/// mongod refuses messages above 48 MB; a `hello` answer is a few hundred bytes.
const MAX_REPLY: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Bson {
    Double(f64),
    String(String),
    Document(Document),
    Array(Vec<Bson>),
    Bool(bool),
    Int32(i32),
    Int64(i64),
    Null,
    /// A type the checks never read (ObjectId, dates, timestamps, binary ...).
    Other(u8),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document(pub Vec<(String, Bson)>);

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: &str, value: Bson) -> Self {
        self.0.push((key.to_owned(), value));
        self
    }

    pub fn get(&self, key: &str) -> Option<&Bson> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Bson::String(s)) => Some(s),
            _ => None,
        }
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key) {
            Some(Bson::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// A number of any BSON numeric type.
    pub fn number(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(Bson::Double(n)) => Some(*n),
            Some(Bson::Int32(n)) => Some(f64::from(*n)),
            Some(Bson::Int64(n)) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![0; 4];
        for (key, value) in &self.0 {
            encode_element(&mut out, key, value);
        }
        out.push(0);
        let size = out.len() as i32;
        out[..4].copy_from_slice(&size.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut reader = Reader { bytes, at: 0 };
        let doc = reader.document()?;
        (reader.at == bytes.len()).then_some(doc)
    }
}

fn cstring(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(text.as_bytes());
    out.push(0);
}

fn encode_element(out: &mut Vec<u8>, key: &str, value: &Bson) {
    let (kind, body): (u8, Vec<u8>) = match value {
        Bson::Double(n) => (0x01, n.to_le_bytes().to_vec()),
        Bson::String(s) => {
            let mut b = ((s.len() + 1) as i32).to_le_bytes().to_vec();
            cstring(&mut b, s);
            (0x02, b)
        }
        Bson::Document(d) => (0x03, d.encode()),
        Bson::Array(items) => {
            let doc = Document(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v.clone()))
                    .collect(),
            );
            (0x04, doc.encode())
        }
        Bson::Bool(b) => (0x08, vec![u8::from(*b)]),
        Bson::Int32(n) => (0x10, n.to_le_bytes().to_vec()),
        Bson::Int64(n) => (0x12, n.to_le_bytes().to_vec()),
        Bson::Null | Bson::Other(_) => (0x0A, Vec::new()),
    };
    out.push(kind);
    cstring(out, key);
    out.extend_from_slice(&body);
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }

    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn cstring(&mut self) -> Option<String> {
        let len = self.bytes.get(self.at..)?.iter().position(|&b| b == 0)?;
        let text = String::from_utf8(self.take(len)?.to_vec()).ok()?;
        self.at += 1;
        Some(text)
    }

    fn string(&mut self) -> Option<String> {
        let len = usize::try_from(self.i32()?).ok()?.checked_sub(1)?;
        let text = String::from_utf8(self.take(len)?.to_vec()).ok()?;
        (self.take(1)? == [0]).then_some(text)
    }

    fn document(&mut self) -> Option<Document> {
        let start = self.at;
        let size = usize::try_from(self.i32()?).ok()?;
        let end = start.checked_add(size)?;
        let mut doc = Document::new();
        while self.at + 1 < end {
            let kind = *self.take(1)?.first()?;
            let key = self.cstring()?;
            doc.0.push((key, self.value(kind)?));
        }
        (self.take(1)? == [0] && self.at == end).then_some(doc)
    }

    fn value(&mut self, kind: u8) -> Option<Bson> {
        Some(match kind {
            0x01 => Bson::Double(f64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            0x02 => Bson::String(self.string()?),
            0x03 => Bson::Document(self.document()?),
            0x04 => Bson::Array(self.document()?.0.into_iter().map(|(_, v)| v).collect()),
            0x08 => Bson::Bool(*self.take(1)?.first()? != 0),
            0x10 => Bson::Int32(self.i32()?),
            0x12 => Bson::Int64(i64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            0x0A => Bson::Null,
            0x06 | 0x7F | 0xFF => Bson::Other(kind),
            0x07 => {
                self.take(12)?;
                Bson::Other(kind)
            }
            0x09 | 0x11 => {
                self.take(8)?;
                Bson::Other(kind)
            }
            0x13 => {
                self.take(16)?;
                Bson::Other(kind)
            }
            0x05 => {
                let len = usize::try_from(self.i32()?).ok()?;
                self.take(len + 1)?;
                Bson::Other(kind)
            }
            0x0B => {
                self.cstring()?;
                self.cstring()?;
                Bson::Other(kind)
            }
            0x0C => {
                self.string()?;
                self.take(12)?;
                Bson::Other(kind)
            }
            0x0D | 0x0E => {
                self.string()?;
                Bson::Other(kind)
            }
            0x0F => {
                let len = usize::try_from(self.i32()?).ok()?;
                self.take(len.checked_sub(4)?)?;
                Bson::Other(kind)
            }
            _ => return None,
        })
    }
}

/// A command mongod answered with `ok: 0`, as the driver's `MongoServerError`.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandError {
    pub code: Option<i32>,
    pub message: String,
}

#[derive(Debug)]
pub enum Failure {
    Io(std::io::Error),
    Command(CommandError),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Command(e) => f.write_str(&e.message),
        }
    }
}

fn invalid(what: &str) -> Failure {
    Failure::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_owned()))
}

/// One connection to one mongod.
pub struct Connection {
    stream: TcpStream,
    next_id: i32,
}

impl Connection {
    pub fn new(stream: TcpStream) -> Self {
        Self { stream, next_id: 1 }
    }

    /// Runs a command against `db` and returns its answer; `ok: 0` is a [`Failure::Command`].
    pub fn command(&mut self, db: &str, command: Document) -> Result<Document, Failure> {
        let body = command.with("$db", Bson::String(db.to_owned())).encode();
        let id = self.next_id;
        self.next_id += 1;
        let mut message = Vec::with_capacity(21 + body.len());
        message.extend_from_slice(&((21 + body.len()) as i32).to_le_bytes());
        message.extend_from_slice(&id.to_le_bytes());
        message.extend_from_slice(&0i32.to_le_bytes());
        message.extend_from_slice(&OP_MSG.to_le_bytes());
        message.extend_from_slice(&0u32.to_le_bytes());
        message.push(0);
        message.extend_from_slice(&body);
        self.stream.write_all(&message).map_err(Failure::Io)?;

        let mut header = [0u8; 16];
        self.stream.read_exact(&mut header).map_err(Failure::Io)?;
        let word = |i: usize| i32::from_le_bytes([header[i], header[i + 1], header[i + 2], header[i + 3]]);
        let length = usize::try_from(word(0)).map_err(|_| invalid("bad message length"))?;
        if !(21..=MAX_REPLY).contains(&length) || word(8) != id || word(12) != OP_MSG {
            return Err(invalid("not an answer to this command"));
        }
        let mut rest = vec![0u8; length - 16];
        self.stream.read_exact(&mut rest).map_err(Failure::Io)?;
        // Flag bits, then a kind-0 section with the body.
        if rest.get(4) != Some(&0) {
            return Err(invalid("unexpected reply section"));
        }
        let doc = rest
            .get(5..)
            .and_then(Document::decode)
            .ok_or_else(|| invalid("bad BSON in the reply"))?;
        if doc.number("ok") == Some(1.0) {
            Ok(doc)
        } else {
            Err(Failure::Command(CommandError {
                code: doc.number("code").map(|c| c as i32),
                message: doc.str("errmsg").unwrap_or("command failed").to_owned(),
            }))
        }
    }
}

/// Reads one `OP_MSG` request off a stream; for the fake mongod in tests.
pub fn read_request(stream: &mut impl Read) -> Option<(i32, Document)> {
    let mut header = [0u8; 16];
    stream.read_exact(&mut header).ok()?;
    let length = usize::try_from(i32::from_le_bytes(header[..4].try_into().ok()?)).ok()?;
    let id = i32::from_le_bytes(header[4..8].try_into().ok()?);
    let mut rest = vec![0u8; length.checked_sub(16)?];
    stream.read_exact(&mut rest).ok()?;
    Some((id, Document::decode(rest.get(5..)?)?))
}

/// Writes one `OP_MSG` answer; for the fake mongod in tests.
pub fn write_reply(stream: &mut impl Write, response_to: i32, doc: &Document) -> std::io::Result<()> {
    let body = doc.encode();
    let mut message = Vec::new();
    message.extend_from_slice(&((21 + body.len()) as i32).to_le_bytes());
    message.extend_from_slice(&0i32.to_le_bytes());
    message.extend_from_slice(&response_to.to_le_bytes());
    message.extend_from_slice(&OP_MSG.to_le_bytes());
    message.extend_from_slice(&0u32.to_le_bytes());
    message.push(0);
    message.extend_from_slice(&body);
    stream.write_all(&message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bson_reads_back_what_it_writes_and_skips_types_it_does_not_read() {
        let doc = Document::new()
            .with("hello", Bson::Int32(1))
            .with("name", Bson::String("rs0 ✓".into()))
            .with("ok", Bson::Double(1.0))
            .with("big", Bson::Int64(1 << 40))
            .with("flag", Bson::Bool(true))
            .with("none", Bson::Null)
            .with(
                "members",
                Bson::Array(vec![Bson::Document(
                    Document::new().with("host", Bson::String("127.0.0.1:27017".into())),
                )]),
            );
        assert_eq!(Document::decode(&doc.encode()), Some(doc));

        // { t: ObjectId(...), when: Date(...), x: 1 }
        let mut raw = vec![0, 0, 0, 0, 0x07, b't', 0];
        raw.extend_from_slice(&[7; 12]);
        raw.extend_from_slice(&[0x09, b'w', b'h', b'e', b'n', 0]);
        raw.extend_from_slice(&[1; 8]);
        raw.extend_from_slice(&[0x10, b'x', 0, 1, 0, 0, 0, 0]);
        let size = raw.len() as i32;
        raw[..4].copy_from_slice(&size.to_le_bytes());
        let read = Document::decode(&raw).unwrap();
        assert_eq!(read.get("t"), Some(&Bson::Other(0x07)));
        assert_eq!(read.number("x"), Some(1.0));
        assert_eq!(Document::decode(&raw[..raw.len() - 1]), None, "truncated");
    }
}
