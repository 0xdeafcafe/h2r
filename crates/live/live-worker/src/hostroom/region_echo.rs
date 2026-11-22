//! Searchers measure one application echo from an active pack or its regional pool.
use super::*;
use futures_util::StreamExt;
use live_proto::matchmaking::REGION_ECHO;

pub(in crate::entry) async fn upgrade(
    live: &Live, env: &Env, req: HttpRequest, region: &str,
) -> Result<Response> {
    if req.method() != http::Method::GET || !crate::relay::websocket_upgrade(req.headers()) {
        return refuse(ErrorCode::Invalid, "the regional echo is a WebSocket");
    }
    if let Err(e) = live.admit_region_echo(live_server::http::bearer(req.headers()), region) {
        return Response::try_from(e.into_response());
    }
    let pool = pool_name(Some(region));
    stub(env, binding(&HostClass::Small), &pool, pool_hint(&pool))?
        .fetch_with_request(echo_request("/pool/echo", false)?)
        .await
}

fn echo_request(path: &str, protocol: bool) -> Result<Request> {
    let headers = Headers::new();
    headers.set("upgrade", "websocket")?;
    if protocol { headers.set("sec-websocket-protocol", REGION_ECHO)?; }
    let mut init = RequestInit::new();
    init.with_headers(headers);
    Request::new_with_init(&format!("https://host-room{path}"), &init)
}

/// Prefer an existing match's path. A pack that is stopping, full of
/// connections, or running an older image leaves the pool's own echo available.
pub(super) async fn pool(room: &Room) -> Result<Response> {
    let pool = room.load_pool().await?;
    if let Some(active) = pool.packs().find(|id| pool.deadline(id).is_some_and(|d| d > now_ms())) {
        let name = room.name();
        let stub = stub(&room.env, room.namespace(), &pack_room(&name, active), pool_hint(&name))?;
        if let Ok(resp) = stub.fetch_with_request(echo_request("/pack/echo", false)?).await
            && resp.status_code() == 101
        {
            return Ok(resp);
        }
    }
    accept(&room.state)
}

pub(super) async fn pack(room: &Room) -> Result<Response> {
    let container = room.container()?;
    let leases = room.load_pack().await?;
    if container.running()
        && let Some(slot) = leases.leases.values().filter(|l| l.deadline_ms > now_ms()).min_by_key(|l| l.tunnel_port)
    {
        let req = echo_request(live_proto::matchmaking::PACK_ECHO_PATH, true)?;
        let resp = forward_tunnel(&container, slot.tunnel_port, req).await?;
        // Prior images accept upgrades on arbitrary paths as *game* connections.
        // Only a verified echo implementation can ever be passed to the client.
        if resp.status_code() == 101 && resp.headers().get("sec-websocket-protocol")?.as_deref() == Some(REGION_ECHO)
            && let Some(socket) = resp.websocket()
        {
            return Response::from_websocket(socket);
        }
    }
    refuse(ErrorCode::Unavailable, "the pack has no regional echo")
}

fn accept(state: &State) -> Result<Response> {
    let pair = worker::WebSocketPair::new()?;
    let server = pair.server;
    server.accept()?;
    state.wait_until(async move {
        if let Ok(mut events) = server.events() {
            tokio::select! {
                message = events.next() => {
                    if let Some(Ok(worker::WebsocketEvent::Message(msg))) = message
                        && msg.as_ref().data() == JsValue::from_str(REGION_ECHO)
                    {
                        let _ = server.send_with_str(REGION_ECHO);
                    }
                }
                _ = worker::Delay::from(std::time::Duration::from_secs(3)) => {}
            }
        }
        let _ = server.close(Some(1000), Some("probe complete"));
    });
    Response::from_websocket(pair.client)
}
