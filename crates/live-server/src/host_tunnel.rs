//! Native WebSocket forwarding to a managed local host's private listener.

use std::time::Duration;

use axum::extract::ws::{
    Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection,
};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use live_proto::{ErrorCode, tunnel};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Message as UpstreamMessage, client::IntoClientRequest};

use super::{ApiResult, Shared, bearer};
use crate::app::{ApiError, invalid};

const MAX_FRAME: usize = tunnel::HEADER_LEN + tunnel::MAX_PAYLOAD;
const IO_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) async fn upgrade(
    State(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> ApiResult<Response> {
    let ws = ws.map_err(|_| invalid("the host tunnel is a WebSocket"))?;
    let (xuid, addr) = live.local_tunnel_admission(bearer(&headers), &id)?;
    let missing = || {
        ApiError::new(
            ErrorCode::Unavailable,
            "the host tunnel port is unavailable",
        )
    };

    let mut request = format!("ws://{addr}/v1/hosts/{id}/tunnel")
        .into_client_request()
        .map_err(|_| missing())?;
    request.headers_mut().insert(
        "x-h2-tunnel-peer",
        xuid.to_string()
            .parse()
            .expect("XUID is a valid header value"),
    );
    let config = WebSocketConfig::default()
        .read_buffer_size(4096)
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let upstream = timeout(IO_TIMEOUT, async {
        let tcp = TcpStream::connect(addr).await?;
        tcp.set_nodelay(true)?;
        tokio_tungstenite::client_async_with_config(request, tcp, Some(config)).await
    })
    .await
    .map_err(|_| missing())?
    .map_err(|_| missing())?
    .0;

    Ok(ws
        .read_buffer_size(4096)
        .max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |client| bridge(client, upstream)))
}

async fn bridge(mut client: WebSocket, mut upstream: WebSocketStream<TcpStream>) {
    // Ping and pong are local to each WebSocket. Only binary datagrams
    // cross the relay, with no decoding or buffering between sockets.
    loop {
        tokio::select! {
            message = client.recv() => match message {
                Some(Ok(Message::Binary(bytes))) => {
                    if !matches!(timeout(IO_TIMEOUT, upstream.send(UpstreamMessage::Binary(bytes))).await, Ok(Ok(()))) {
                        break;
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => break,
            },
            message = upstream.next() => match message {
                Some(Ok(UpstreamMessage::Binary(bytes))) => {
                    if !matches!(timeout(IO_TIMEOUT, client.send(Message::Binary(bytes))).await, Ok(Ok(()))) {
                        break;
                    }
                }
                Some(Ok(UpstreamMessage::Ping(_))) => {
                    if !matches!(timeout(IO_TIMEOUT, upstream.flush()).await, Ok(Ok(()))) {
                        break;
                    }
                }
                Some(Ok(UpstreamMessage::Pong(_))) => {}
                _ => break,
            },
        }
    }
    let _ = tokio::join!(
        timeout(IO_TIMEOUT, client.send(Message::Close(None))),
        timeout(IO_TIMEOUT, upstream.close(None)),
    );
}
