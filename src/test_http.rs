//! Local HTTP/1.1 server for tests, one response per connection.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves on 127.0.0.1 until the test's runtime stops. `respond` gets the
/// request line and headers and returns the whole response, or `None` to hang
/// up without one (a transport error for the client).
pub async fn serve<Respond, Reply>(respond: Respond) -> SocketAddr
where
    Respond: Fn(String) -> Reply + Send + Sync + 'static,
    Reply: Future<Output = Option<Vec<u8>>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let respond = Arc::new(respond);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let respond = respond.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 8192];
                let mut count = 0;
                while !buffer[..count].windows(4).any(|window| window == b"\r\n\r\n") {
                    match socket.read(&mut buffer[count..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => count += read,
                    }
                }
                let request = String::from_utf8_lossy(&buffer[..count]).into_owned();
                if let Some(response) = respond(request).await {
                    // The client may hang up first (a cancelled request).
                    socket.write_all(&response).await.ok();
                }
            });
        }
    });
    addr
}

pub fn path(request: &str) -> &str {
    request.split_whitespace().nth(1).unwrap_or("/")
}

/// `headers` are extra `name: value\r\n` lines.
pub fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes =
        format!("HTTP/1.1 {status}\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n", body.len())
            .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
