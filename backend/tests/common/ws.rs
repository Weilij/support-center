//! Shared WebSocket test helpers: real listener + tokio-tungstenite client.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::TestApp;

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Serve the app on a real port; HTTP assertions still go through the shared
/// router (same AppState/hub), WS connections through the bound listener.
pub async fn serve(app: &TestApp) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

pub fn mint(sub: &str, role: &str, ttl_secs: i64) -> String {
    use mcss_backend::domain::auth::tokens::{self, Claims};
    let claims = Claims::new(sub, role, "access", ttl_secs);
    tokens::sign(&claims, "test-secret").unwrap()
}

/// Open a websocket against an arbitrary path (path includes the query).
pub async fn ws_connect(addr: SocketAddr, path_and_query: &str) -> Result<Ws, WsError> {
    let url = format!("ws://{addr}{path_and_query}");
    tokio_tungstenite::connect_async(url)
        .await
        .map(|(ws, _)| ws)
}

/// Expect the handshake to be rejected; return (status, body).
pub async fn connect_rejected(addr: SocketAddr, path_and_query: &str) -> (u16, Value) {
    match ws_connect(addr, path_and_query).await {
        Err(WsError::Http(resp)) => {
            let status = resp.status().as_u16();
            let body = resp
                .body()
                .as_ref()
                .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                .unwrap_or(Value::Null);
            (status, body)
        }
        Ok(_) => panic!("handshake unexpectedly accepted for {path_and_query}"),
        Err(e) => panic!("unexpected websocket error: {e}"),
    }
}

pub async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("stream ended")
            .expect("websocket error");
        if let Message::Text(t) = msg {
            return serde_json::from_str(t.as_str()).expect("frame is not JSON");
        }
    }
}

pub async fn wait_for_event(ws: &mut Ws, event: &str) -> Value {
    for _ in 0..30 {
        let v = next_json(ws).await;
        if v["type"] == event {
            return v;
        }
    }
    panic!("did not receive event {event}");
}

pub async fn send_json(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// Assert that no frame arrives within the window (bounded, deterministic
/// enough for ignored-frame contracts).
pub async fn expect_silence(ws: &mut Ws, window: Duration) {
    let got = tokio::time::timeout(window, ws.next()).await;
    if let Ok(Some(Ok(Message::Text(t)))) = got {
        panic!("expected silence but received frame: {t}");
    }
}
