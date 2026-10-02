//! Minimal HTTP/1.1 transport for the A2A JSON-RPC binding.
//!
//! One request per connection (`Connection: close`), no chunked bodies, no
//! keep-alive, no pipelining: the smallest surface that serves the binding.
//! Fail-closed checks, in order: header size, read deadline, `Host` must be a
//! loopback name (DNS-rebinding defense), `Transfer-Encoding` refused,
//! `Content-Length` required and bounded before any body byte is read,
//! `Content-Type: application/json` required on POST (a browser cannot send
//! that cross-origin without a preflight, which this server never grants).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, broadcast};

use super::task::{EVENTS_PER_RUN_MAX, TaskEvent};
use super::{
    Agent, CANCEL_ACK_WAIT, REQUEST_BODY_BYTES_MAX, Reply, codes, error_envelope, result_envelope,
    rpc_err,
};
use crate::LumenError;

/// Concurrent connections; excess connections are closed immediately.
pub(crate) const CONNECTIONS_MAX: usize = 32;
const HEADER_BYTES_MAX: usize = 16 * 1024;
const HEADER_COUNT_MAX: usize = 64;
const READ_CHUNK_BYTES: usize = 4096;
const REQUEST_READ_DEADLINE: Duration = Duration::from_secs(10);
const WRITE_DEADLINE: Duration = Duration::from_secs(10);
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);
const CARD_PATH: &str = "/.well-known/agent-card.json";
const RPC_PATH: &str = "/";

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug)]
struct HttpError {
    status: u16,
    message: &'static str,
}

fn http_err(status: u16, message: &'static str) -> HttpError {
    HttpError { status, message }
}

pub(crate) async fn serve(listener: TcpListener, agent: Agent) -> Result<(), LumenError> {
    let permits = Arc::new(Semaphore::new(CONNECTIONS_MAX));
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                // Typically fd exhaustion; back off instead of spinning.
                tracing::warn!("a2a accept failed: {e}");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
        };
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let agent = agent.clone();
        tokio::spawn(async move {
            handle_connection(stream, agent).await;
            drop(permit);
        });
    }
}

async fn handle_connection(mut stream: TcpStream, agent: Agent) {
    let read = tokio::time::timeout(REQUEST_READ_DEADLINE, read_request(&mut stream)).await;
    let request = match read {
        Ok(Ok(request)) => request,
        Ok(Err(err)) => return write_error(&mut stream, &err).await,
        Err(_) => return write_error(&mut stream, &http_err(408, "request timeout")).await,
    };
    if let Err(err) = check_host(&request) {
        return write_error(&mut stream, &err).await;
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", CARD_PATH) => {
            let body = agent.card().to_string();
            write_response(&mut stream, 200, "application/json", body.as_bytes()).await;
        }
        ("POST", RPC_PATH) => handle_rpc(&mut stream, &agent, &request).await,
        (_, CARD_PATH | RPC_PATH) => write_error(&mut stream, &http_err(405, "method")).await,
        _ => write_error(&mut stream, &http_err(404, "not found")).await,
    }
}

async fn handle_rpc(stream: &mut TcpStream, agent: &Agent, request: &HttpRequest) {
    let json_type = request
        .header("content-type")
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"));
    if !json_type {
        return write_error(stream, &http_err(415, "content-type must be application/json")).await;
    }
    match agent.dispatch(&request.body, true).await {
        Reply::Json(envelope) => {
            let body = envelope.to_string();
            write_response(stream, 200, "application/json", body.as_bytes()).await;
        }
        Reply::Stream { id, first, events } => {
            let limit = agent.shared.task_deadline + CANCEL_ACK_WAIT;
            write_stream(stream, &id, first, events, limit).await;
        }
    }
}

fn check_host(request: &HttpRequest) -> Result<(), HttpError> {
    let host = request.header("host").ok_or_else(|| http_err(400, "host header required"))?;
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    let loopback = name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" || name == "::1";
    if loopback { Ok(()) } else { Err(http_err(403, "host must be a loopback name")) }
}

/// Read headers (bounded), then exactly `Content-Length` body bytes (bounded
/// before reading). The caller applies the overall read deadline.
async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, HttpError> {
    let mut buf: Vec<u8> = Vec::with_capacity(READ_CHUNK_BYTES);
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > HEADER_BYTES_MAX {
            return Err(http_err(431, "headers too large"));
        }
        let read = stream.read(&mut chunk).await.map_err(|_| http_err(400, "read failed"))?;
        if read == 0 {
            return Err(http_err(400, "connection closed mid-request"));
        }
        buf.extend_from_slice(&chunk[..read]);
    };
    if header_end > HEADER_BYTES_MAX {
        return Err(http_err(431, "headers too large"));
    }
    let head = std::str::from_utf8(&buf[..header_end]).map_err(|_| http_err(400, "non-utf8"))?;
    let (method, path, headers) = parse_head(head)?;
    if headers.iter().any(|(n, _)| n == "transfer-encoding") {
        return Err(http_err(501, "transfer-encoding is not supported"));
    }
    let body_len = content_length(&headers, &method)?;
    let already = &buf[header_end + 4..];
    if already.len() > body_len {
        return Err(http_err(400, "body longer than content-length"));
    }
    let mut body = Vec::with_capacity(body_len);
    body.extend_from_slice(already);
    body.resize(body_len, 0);
    let filled = already.len();
    stream
        .read_exact(&mut body[filled..])
        .await
        .map_err(|_| http_err(400, "body shorter than content-length"))?;
    Ok(HttpRequest { method, path, headers, body })
}

type Head = (String, String, Vec<(String, String)>);

fn parse_head(head: &str) -> Result<Head, HttpError> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut words = request_line.split(' ');
    let (Some(method), Some(path), Some(version), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Err(http_err(400, "malformed request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(http_err(505, "http/1.x only"));
    }
    let mut headers = Vec::new();
    for line in lines {
        if headers.len() >= HEADER_COUNT_MAX {
            return Err(http_err(431, "too many headers"));
        }
        let (name, value) = line.split_once(':').ok_or_else(|| http_err(400, "bad header"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok((method.to_string(), path.to_string(), headers))
}

fn content_length(headers: &[(String, String)], method: &str) -> Result<usize, HttpError> {
    let mut values = headers.iter().filter(|(n, _)| n == "content-length");
    let first = values.next();
    if values.next().is_some() {
        return Err(http_err(400, "duplicate content-length"));
    }
    let Some((_, raw)) = first else {
        if method == "POST" {
            return Err(http_err(411, "content-length required"));
        }
        return Ok(0);
    };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(http_err(400, "bad content-length"));
    }
    match raw.parse::<usize>() {
        Ok(len) if len <= REQUEST_BODY_BYTES_MAX => Ok(len),
        _ => Err(http_err(413, "request body too large")),
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}

/// Best-effort write: a vanished client is not a server error.
async fn write_bytes(stream: &mut TcpStream, bytes: &[u8]) -> bool {
    matches!(tokio::time::timeout(WRITE_DEADLINE, stream.write_all(bytes)).await, Ok(Ok(())))
}

async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    if write_bytes(stream, head.as_bytes()).await {
        write_bytes(stream, body).await;
    }
}

async fn write_error(stream: &mut TcpStream, err: &HttpError) {
    let body = json!({"error": err.message}).to_string();
    write_response(stream, err.status, "application/json", body.as_bytes()).await;
}

async fn write_sse(stream: &mut TcpStream, envelope: &Value) -> bool {
    write_bytes(stream, format!("data: {envelope}\n\n").as_bytes()).await
}

fn settled(task: &Value) -> bool {
    matches!(
        task.pointer("/status/state").and_then(Value::as_str),
        Some("completed" | "failed" | "canceled" | "rejected" | "input-required")
    )
}

/// SSE: the task snapshot, then each event until one is final. Bounded by
/// `limit` wall time and the channel's bounded event count. A client that
/// disconnects only ends its stream; the task keeps running.
async fn write_stream(
    stream: &mut TcpStream,
    id: &Value,
    first: Value,
    mut events: broadcast::Receiver<TaskEvent>,
    limit: Duration,
) {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                Cache-Control: no-store\r\nConnection: close\r\n\r\n";
    let done = settled(&first);
    if !write_bytes(stream, head.as_bytes()).await
        || !write_sse(stream, &result_envelope(id, first)).await
        || done
    {
        return;
    }
    let deadline = tokio::time::Instant::now() + limit;
    for _ in 0..EVENTS_PER_RUN_MAX {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(event)) => {
                let is_final = event.is_final;
                if !write_sse(stream, &result_envelope(id, event.result)).await || is_final {
                    return;
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                let err = rpc_err(codes::INTERNAL_ERROR, "event stream lagged; use tasks/get");
                write_sse(stream, &error_envelope(id, &err)).await;
                return;
            }
            Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return,
        }
    }
}
