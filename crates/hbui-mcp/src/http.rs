//! MCP over streamable HTTP, the transport that lets a person and an agent
//! share one live UI.
//!
//! Stdio would have the agent's client own the process and its stdout, which
//! is exactly what the person's terminal needs. A socket leaves the terminal
//! alone: the application runs in it as usual and also listens here.
//!
//! Only the request/response half of the transport is implemented: `POST
//! /mcp` answers with one JSON body. This server never has anything to say
//! unprompted, so the server-initiated SSE stream (`GET /mcp`) is refused with
//! 405, which the specification allows.
//!
//! Bound to loopback only, and requests carrying a non-local `Origin` are
//! refused: this hands control of the person's UI to whoever connects, and a
//! web page must not be able to be that someone.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Arc;

use serde_json::Value;

use crate::server::Server;
use crate::wire::{Request, Response, PARSE_ERROR};

/// Largest request body accepted. A semantic action is a few hundred bytes.
const MAX_BODY: usize = 1 << 20;

/// Listen on `127.0.0.1:port` (0 for any free port) and serve on a background
/// thread. Returns the address actually bound.
pub fn serve_http(server: Arc<Server>, port: u16) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
    let addr = listener.local_addr()?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let server = server.clone();
            std::thread::spawn(move || {
                let _ = connection(&server, stream);
            });
        }
    });
    Ok(addr)
}

/// Serve requests on one connection until the client closes it.
fn connection(server: &Server, stream: TcpStream) -> io::Result<()> {
    stream.set_nodelay(true).ok();
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut out = stream;
    while let Some(req) = read_request(&mut reader)? {
        let foreign = req.header("origin").is_some_and(|o| !is_local_origin(o));
        match (
            req.method.as_str(),
            req.path.split('?').next().unwrap_or(""),
        ) {
            _ if foreign => respond(
                &mut out,
                "403 Forbidden",
                "application/json",
                r#"{"error":"non-local origin"}"#,
            )?,
            ("POST", "/mcp") => {
                let response = match serde_json::from_slice::<Request>(&req.body) {
                    Ok(rpc) => server.handle(rpc),
                    Err(e) => Some(Response::error(
                        Value::Null,
                        PARSE_ERROR,
                        format!("parse error: {e}"),
                    )),
                };
                match response {
                    Some(r) => {
                        let body = serde_json::to_string(&r).unwrap_or_default();
                        respond(&mut out, "200 OK", "application/json", &body)?;
                    }
                    None => respond(&mut out, "202 Accepted", "application/json", "")?,
                }
            }
            (_, "/mcp") => respond(&mut out, "405 Method Not Allowed", "application/json", "")?,
            _ => respond(
                &mut out,
                "404 Not Found",
                "application/json",
                r#"{"error":"POST /mcp"}"#,
            )?,
        }
    }
    Ok(())
}

fn is_local_origin(origin: &str) -> bool {
    let host = origin
        .split("://")
        .nth(1)
        .unwrap_or(origin)
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Read one request, or `None` at a clean end of the connection.
fn read_request(reader: &mut impl BufRead) -> io::Result<Option<HttpRequest>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad request line",
        ));
    };
    let (method, path) = (method.to_string(), path.to_string());

    let mut headers = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let req = HttpRequest {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let len: usize = req
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    Ok(Some(HttpRequest { body, ..req }))
}

fn respond(out: &mut impl Write, status: &str, content_type: &str, body: &str) -> io::Result<()> {
    write!(
        out,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_origins_are_local() {
        assert!(is_local_origin("http://localhost:3000"));
        assert!(is_local_origin("http://127.0.0.1"));
        assert!(!is_local_origin("https://evil.example"));
        assert!(!is_local_origin("http://localhost.evil.example"));
    }

    #[test]
    fn reads_a_request_with_a_body() {
        let raw = "POST /mcp HTTP/1.1\r\nContent-Length: 4\r\nOrigin: x\r\n\r\nabcd";
        let req = read_request(&mut raw.as_bytes()).unwrap().unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.body, b"abcd");
        assert_eq!(req.header("origin"), Some("x"));
    }
}
