use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const MAX_HEADER: usize = 8 * 1024;
const MAX_CONTENT: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("LSP transport failed")]
    Io(#[source] std::io::Error),
    #[error("LSP framing error: {0}")]
    Protocol(&'static str),
    #[error("LSP transport is unavailable after interrupted or failed I/O")]
    Unavailable,
}

impl From<std::io::Error> for TransportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Single mutable owner; an interrupted read or write permanently fences this transport.
/// A timeout or dropped future does not acknowledge native process termination.
pub struct Framed<R, W> {
    reader: BufReader<R>,
    writer: W,
    available: bool,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Framed<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            available: true,
        }
    }

    fn begin_io(&mut self) -> Result<(), TransportError> {
        if !self.available {
            return Err(TransportError::Unavailable);
        }
        // Set before any await: disposing the future must not reopen a partial frame.
        self.available = false;
        Ok(())
    }

    /// Clean EOF returns None and fences reuse; truncated frames return an error.
    pub async fn read(&mut self) -> Result<Option<Value>, TransportError> {
        self.begin_io()?;
        let Some(header) = read_header(&mut self.reader).await? else {
            return Ok(None);
        };
        let length = content_length(&header)?;
        let mut body = vec![0; length];
        self.reader.read_exact(&mut body).await?;
        let value = serde_json::from_slice(&body)
            .map_err(|_| TransportError::Protocol("invalid JSON content"))?;
        validate_message(&value)?;
        self.available = true;
        Ok(Some(value))
    }

    pub async fn write(&mut self, message: &Value) -> Result<(), TransportError> {
        // Validate before admission: a refused local message has sent no bytes.
        validate_message(message)?;
        let body = serde_json::to_vec(message)
            .map_err(|_| TransportError::Protocol("invalid JSON content"))?;
        if body.len() > MAX_CONTENT {
            return Err(TransportError::Protocol("content limit exceeded"));
        }
        self.begin_io()?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer.write_all(header.as_bytes()).await?;
        self.writer.write_all(&body).await?;
        self.writer.flush().await?;
        self.available = true;
        Ok(())
    }
}

async fn read_header<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Vec<u8>>, TransportError> {
    let mut header = Vec::new();
    while header.len() < MAX_HEADER {
        let mut byte = [0];
        if reader.read(&mut byte).await? == 0 {
            return if header.is_empty() {
                Ok(None)
            } else {
                Err(TransportError::Protocol("truncated header"))
            };
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            return Ok(Some(header));
        }
    }
    Err(TransportError::Protocol("header limit exceeded"))
}

fn content_length(header: &[u8]) -> Result<usize, TransportError> {
    if !header.is_ascii() {
        return Err(TransportError::Protocol("non-ASCII header"));
    }
    let header =
        std::str::from_utf8(header).map_err(|_| TransportError::Protocol("invalid header"))?;
    let mut length = None;
    let mut content_type = false;
    for line in header[..header.len() - 4].split("\r\n") {
        let (name, value) = header_field(line)?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(TransportError::Protocol("duplicate content length"));
            }
            length = Some(parse_length(value)?);
        } else if name.eq_ignore_ascii_case("Content-Type") {
            if content_type {
                return Err(TransportError::Protocol("duplicate content type"));
            }
            validate_content_type(value)?;
            content_type = true;
        }
    }
    length.ok_or(TransportError::Protocol("missing content length"))
}

fn header_field(line: &str) -> Result<(&str, &str), TransportError> {
    let (name, value) = line
        .split_once(':')
        .ok_or(TransportError::Protocol("invalid header field"))?;
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(TransportError::Protocol("invalid header name"));
    }
    if value.bytes().any(|b| b.is_ascii_control() && b != b'\t') {
        return Err(TransportError::Protocol("invalid header value"));
    }
    Ok((name, value.trim()))
}

fn parse_length(value: &str) -> Result<usize, TransportError> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TransportError::Protocol("invalid content length"));
    }
    let length = value
        .parse::<usize>()
        .map_err(|_| TransportError::Protocol("invalid content length"))?;
    if length == 0 || length > MAX_CONTENT {
        return Err(TransportError::Protocol("content limit exceeded"));
    }
    Ok(length)
}

fn validate_content_type(value: &str) -> Result<(), TransportError> {
    let mut fields = value.split(';');
    if !fields
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("application/vscode-jsonrpc")
    {
        return Err(TransportError::Protocol("unsupported content type"));
    }
    let mut charset = false;
    for field in fields {
        let (name, value) = field
            .trim()
            .split_once('=')
            .ok_or(TransportError::Protocol("invalid content type parameter"))?;
        if !name.trim().eq_ignore_ascii_case("charset") || charset {
            return Err(TransportError::Protocol("invalid content type parameter"));
        }
        let value = value.trim().trim_matches('"');
        if !value.eq_ignore_ascii_case("utf-8") && !value.eq_ignore_ascii_case("utf8") {
            return Err(TransportError::Protocol("unsupported charset"));
        }
        charset = true;
    }
    Ok(())
}

fn validate_message(value: &Value) -> Result<(), TransportError> {
    if !value.is_object() || value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(TransportError::Protocol("expected JSON-RPC 2.0 object"));
    }
    Ok(())
}
