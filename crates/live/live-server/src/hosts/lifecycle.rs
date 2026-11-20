use super::*;
use live_proto::hosts::Transport;

/// What reserving a lease produced.
enum Reserved {
    /// A fresh `Requested` lease, its event recorded and its boot
    /// credential minted.
    Fresh {
        lease: HostLease,
        provider: Arc<dyn HostProvider>,
        boot: HostBoot,
    },
    /// The assignment already holds this same active lease.
    Existing(HostLease),
}

impl Live {
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    pub(crate) fn install_native_host_provider(&self, data_root: &std::path::Path) {
        let env = |k: &str| std::env::var(k).ok();
        let mut slot = self.hosts.provider.write().unwrap();
        if slot.is_some() {
            return;
        }
        *slot = match env("LIVE_HOST_PROVIDER").as_deref() {
            None | Some("local") => Some(Arc::new(local::LocalProvider::new(data_root))),
            Some("fly") => match fly::FlyProvider::from_env(&env) {
                Ok(provider) => Some(Arc::new(provider)),
                Err(_) => {
                    eprintln!("live: Fly host provider is not configured");
                    None
                }
            },
            Some(_) => None,
        };
    }

    pub fn max_hosts(&self) -> usize {
        self.hosts.config.read().unwrap().max_hosts
    }

    /// Trusted lookup for a retry of the same match or sim run.
    pub fn host_for_assignment(&self, assignment_id: &str) -> Option<HostView> {
        self.hosts
            .registry
            .read()
            .unwrap()
            .leases
            .values()
            .find(|r| r.lease.assignment_id == assignment_id)
            .cloned()
    }

    /// Reserve a lease exactly as [`Self::request_host`] does: the retry
    /// check, budget and slot limits, validation, the `HostRequested` event
    /// and the boot credential. `launch` reserves the launching slot a
    /// provider start needs; without it the lease waits for the operator's
    /// own container to register.
    async fn reserve_host(&self, mut lease: HostLease, launch: bool) -> ApiResult<Reserved> {
        // Fly hosts use direct UDP. Normalize before comparing retries or
        // writing the lease, so matchmaking and the supervisor agree.
        if lease.provider == ProviderKind::Fly {
            lease.transport = Transport::Udp;
        }
        let _write = self.hosts.writes.lock().await;
        let version = self.hosts.version();
        if let Some(row) = self.host_for_assignment(&lease.assignment_id) {
            lease.id.clone_from(&row.lease.id);
            lease.created_ms = row.lease.created_ms;
            if !row.active() || row.lease != lease {
                return Err(conflict(
                    "assignment already has a different or finished host",
                ));
            }
            drop(_write);
            return Ok(Reserved::Existing(lease));
        }

        let now = now_ms();
        if lease.assignment_id.is_empty()
            || lease.map.is_empty()
            || !(1..=16).contains(&lease.max_players)
            || lease.roster.len() + lease.bots.len() > usize::from(lease.max_players)
            || lease.max_match_s == 0
            || lease.idle_timeout_s == 0
        {
            return Err(invalid("invalid host assignment, seats or duration"));
        }
        let provider = self.hosts.provider_for(&lease)?;
        lease.created_ms = now;
        let pack_linger = launch && lease.provider == ProviderKind::Cloudflare;
        if !self.hosts.costs.can_start(&lease, pack_linger) {
            provider.log_budget_refusal(&format!(
                "live: monthly dedicated host budget refused assignment {} (spent ${:.4}, cap ${:.2})",
                lease.assignment_id,
                self.hosts.costs.month_to_date(now),
                self.hosts.costs.budget_usd_month().unwrap_or_default(),
            ));
            return Err(unavailable(
                "monthly dedicated host budget cannot cover another full match",
            ));
        }
        let config = self.hosts.config.read().unwrap().clone();
        let running = self.hosts.registry.read().unwrap().occupied();
        if running >= config.max_hosts {
            return Err(ApiError::new(
                ErrorCode::RateLimited,
                "all dedicated host slots are occupied",
            ));
        }
        let live_url = if config.live_url.is_empty() {
            self.policy()
                .public
                .get()
                .or_else(|| {
                    (lease.provider == ProviderKind::Local)
                        .then(|| format!("http://{}", crate::DEFAULT_LISTEN))
                })
                .ok_or_else(|| unavailable("dedicated host Live URL is not configured"))?
        } else {
            config.live_url
        };
        if config.data_url.is_empty()
            || config.data_sha256.len() != 64
            || !config.data_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(unavailable("dedicated host data bundle is not configured"));
        }
        lease.id = HostLease::new_id();
        let provider_ref = if launch {
            provider.reserved_ref(&lease)
        } else {
            None
        };
        let boot = HostBoot {
            live_url,
            data_url: config.data_url,
            data_sha256: config.data_sha256,
            archive: live_proto::hosts::HostArchive::parse(&config.archive)
                .map_err(|_| unavailable("LIVE_HOST_ARCHIVE must be dual or film"))?,
            token: self.issue_host_token(&lease),
        };
        self.host_event(
            version,
            HOST_REQUESTED,
            HostRequested {
                lease: lease.clone(),
                provider_ref,
                pack_linger,
            },
        )
        .await?;
        let accepted = self.matchmaking.lock().unwrap().host_requested(&lease);
        if !accepted {
            let row = self.require_host(&lease.id)?;
            self.record_host_end(self.hosts.version(), &row, EndReason::Stopped, 0, now_ms())
                .await?;
            return Err(conflict("the matchmaking assignment has expired"));
        }
        if launch {
            self.hosts
                .registry
                .write()
                .unwrap()
                .launching
                .insert(lease.id.clone(), None);
        }
        Ok(Reserved::Fresh {
            lease,
            provider,
            boot,
        })
    }

    pub async fn request_host(&self, lease: HostLease) -> ApiResult<HostLease> {
        let (lease, provider, boot) = match self.reserve_host(lease, true).await? {
            Reserved::Fresh {
                lease,
                provider,
                boot,
            } => (lease, provider, boot),
            // The provider is already starting this assignment's host.
            Reserved::Existing(lease) => return self.wait_for_host_start(&lease.id).await,
        };

        // The allocation outlives a dropped HTTP request. A slow start can
        // return a billable resource after the reaper already ended its lease.
        // Keep the task to receive its reference and dispose of it in that case.
        let weak = self
            .hosts
            .live
            .get()
            .expect("Live attached at startup")
            .clone();
        let (reply, result) = tokio::sync::oneshot::channel();
        let requested = lease.clone();
        crate::rt::spawn(async move {
            let started = provider.start(&requested, &boot).await;
            let Some(live) = weak.upgrade() else {
                if let Ok(id) = started {
                    let _ = provider.stop(&requested.id, &id).await;
                }
                return;
            };
            let result = live
                .finish_host_start(&requested.id, &*provider, started)
                .await;
            let _ = reply.send(result);
        });
        let deadline = start_deadline(&lease);
        let remaining = deadline.saturating_sub(now_ms()).max(0) as u64;
        match crate::rt::timeout(Duration::from_millis(remaining), result).await {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => return Err(unavailable("dedicated host launch interrupted")),
            Err(_) => {
                return self
                    .expire_host_start(&lease.id, deadline.max(now_ms()))
                    .await;
            }
        }
        Ok(lease)
    }

    // A duplicate request or unusually fast registering process can arrive
    // before start returns its reference. Subscribe before reading to avoid
    // missing a concurrent HostBooting, and leave the writer free to append it.
    async fn wait_for_host_start(&self, id: &str) -> ApiResult<HostLease> {
        let mut applied = self.applied();
        loop {
            let row = self.require_host(id)?;
            match row.state {
                HostState::Booting | HostState::Joinable | HostState::Playing => {
                    return Ok(row.lease);
                }
                HostState::Failed | HostState::Ended => {
                    return Err(conflict("host lease already ended"));
                }
                HostState::Requested => {}
            }
            let remaining = start_deadline(&row.lease).saturating_sub(now_ms());
            if remaining <= 0
                || !matches!(
                    crate::rt::timeout(Duration::from_millis(remaining as u64), applied.changed())
                        .await,
                    Ok(Ok(()))
                )
            {
                return Err(unavailable("dedicated host boot timed out"));
            }
        }
    }

    // The provider reply and this timeout can wake in either order. A lobby
    // already registered at the deadline must still be handed to its cohort.
    pub(super) async fn expire_host_start(&self, id: &str, at: i64) -> ApiResult<HostLease> {
        let _write = self.hosts.writes.lock().await;
        let version = self.hosts.version();
        let row = self.require_host(id)?;
        if matches!(row.state, HostState::Joinable | HostState::Playing) {
            return Ok(row.lease);
        }
        if let Some((due, reason)) = row.deadline()
            && at >= due
        {
            self.stop_host_locked(version, &row, reason, row.players, at)
                .await?;
        }
        Err(unavailable("dedicated host boot timed out"))
    }

    async fn finish_host_start(
        &self,
        id: &str,
        provider: &dyn HostProvider,
        started: Result<String, HostError>,
    ) -> ApiResult<()> {
        let _write = self.hosts.writes.lock().await;
        let version = self.hosts.version();
        let row = self.require_host(id)?;
        let now = now_ms();
        let Ok(provider_ref) = started else {
            self.hosts.registry.write().unwrap().launching.remove(id);
            if row.state == HostState::Requested {
                let reason = row
                    .deadline()
                    .filter(|(due, _)| *due <= now)
                    .map_or(EndReason::Crashed, |(_, reason)| reason);
                self.record_host_end(version, &row, reason, 0, now).await?;
            }
            return Err(unavailable(
                "dedicated host provider failed to start the match",
            ));
        };
        // If the provider's compensating stop fails, the sweep retries with
        // this same reference rather than launching another process.
        self.hosts
            .registry
            .write()
            .unwrap()
            .launching
            .insert(id.into(), Some(provider_ref.clone()));
        if row.state != HostState::Requested || row.deadline().is_some_and(|(at, _)| at <= now) {
            provider
                .stop(id, &provider_ref)
                .await
                .map_err(|_| unavailable("late host could not be stopped"))?;
            self.hosts.registry.write().unwrap().launching.remove(id);
            if let Some((_, reason)) = row.deadline() {
                self.record_host_end(version, &row, reason, 0, now).await?;
            }
            return Err(conflict("dedicated host launch is no longer needed"));
        }
        let boot = HostBooting {
            lease_id: id.into(),
            provider_ref: provider_ref.clone(),
        };
        if let Err(error) = self.host_event(version, HOST_BOOTING, boot).await {
            // Never leave a machine running without a durable provider reference.
            provider
                .stop(id, &provider_ref)
                .await
                .map_err(|_| unavailable("unrecorded host could not be stopped"))?;
            self.hosts.registry.write().unwrap().launching.remove(id);
            return Err(error);
        }
        self.hosts.registry.write().unwrap().launching.remove(id);
        Ok(())
    }

    fn check_host_bearer(
        &self,
        bearer: Option<&str>,
        scope: &str,
        lease_id: &str,
    ) -> ApiResult<()> {
        if self.verify_host_token(bearer, scope)? != lease_id {
            return Err(forbidden("host credential belongs to a different lease"));
        }
        if !self.hosts.reports.allow(lease_id) {
            return Err(ApiError::new(
                ErrorCode::RateLimited,
                "too many host requests",
            ));
        }
        Ok(())
    }

    pub async fn host_register(
        &self,
        bearer: Option<&str>,
        lease_id: &str,
        endpoint: HostEndpoint,
    ) -> ApiResult<HostLease> {
        self.check_host_bearer(bearer, crate::host_identity::HOST_REGISTER, lease_id)?;
        // A lease whose provider start is still in flight first waits for
        // its HostBooting. A manual lease (an admin's `launch: false`) has
        // no provider: its container registers straight out of Requested.
        if self.hosts.launching(lease_id) {
            self.wait_for_host_start(lease_id).await?;
        }
        let _write = self.hosts.writes.lock().await;
        let version = self.hosts.version();
        let row = self.require_host(lease_id)?;
        if matches!(row.state, HostState::Joinable | HostState::Playing)
            && row.endpoint.as_ref() == Some(&endpoint)
        {
            return Ok(row.lease);
        }
        if matches!(row.state, HostState::Ended | HostState::Failed) {
            return Err(conflict("host lease already ended"));
        }
        if !matches!(row.state, HostState::Booting | HostState::Requested) {
            return Err(conflict("only a booting host can register its endpoint"));
        }
        let now = now_ms();
        if row.deadline().is_some_and(|(at, _)| at <= now) {
            return Err(conflict("host boot deadline has passed"));
        }
        super::endpoint::validate(&endpoint, &row.lease.transport)?;
        self.host_event(
            version,
            HOST_READY,
            HostReady {
                lease_id: lease_id.into(),
                endpoint,
                boot_ms: elapsed(&row.lease, now),
            },
        )
        .await?;
        Ok(row.lease)
    }

    pub async fn host_report(
        &self,
        bearer: Option<&str>,
        lease_id: &str,
        state: HostState,
        players: u8,
        reason: Option<EndReason>,
    ) -> ApiResult<HostView> {
        self.check_host_bearer(bearer, crate::host_identity::HOST_REPORT, lease_id)?;
        let _write = self.hosts.writes.lock().await;
        let version = self.hosts.version();
        let row = self.require_host(lease_id)?;
        if players > row.lease.max_players {
            return Err(invalid("player count exceeds the lease capacity"));
        }
        match state {
            HostState::Playing => {
                if !matches!(row.state, HostState::Joinable | HostState::Playing) {
                    return Err(conflict("host is not ready to play"));
                }
                if reason.is_some() {
                    return Err(invalid("only an ended report has an end reason"));
                }
                if row.deadline().is_some_and(|(at, _)| at <= now_ms()) {
                    return Err(conflict("host lease deadline has passed"));
                }
                self.host_event(
                    version,
                    HOST_PLAYING,
                    HostPlaying {
                        lease_id: lease_id.into(),
                        players,
                    },
                )
                .await?;
            }
            HostState::Ended => {
                let reason = reason.unwrap_or(EndReason::MatchOver);
                if !row.active() {
                    return if row.reason.as_ref() == Some(&reason) {
                        Ok(row)
                    } else {
                        Err(conflict("host lease already ended"))
                    };
                }
                // The supervisor still has to receive this response and tear
                // down the game. A replayable sweep cleans up a lingering process.
                self.record_host_end(version, &row, reason, players, now_ms())
                    .await?;
            }
            _ => return Err(invalid("host may report only playing or ended")),
        }
        self.require_host(lease_id)
    }

    pub fn host_get(&self, bearer: Option<&str>, lease_id: &str) -> ApiResult<HostView> {
        let who = self.authenticate(bearer, lxm::READ)?;
        let row = self.require_host(lease_id)?;
        let member =
            matches!(who, Author::Account { xuid, .. } if row.lease.roster.contains(&xuid));
        if !member && !self.admins.is_admin(&who, &self.model.read()) {
            return Err(forbidden(
                "reading this host requires its roster or an admin",
            ));
        }
        Ok(row)
    }

    #[cfg(feature = "native")]
    pub(crate) fn local_tunnel_admission(
        &self,
        bearer: Option<&str>,
        id: &str,
    ) -> ApiResult<(live_proto::Xuid, std::net::SocketAddr)> {
        let Author::Account { xuid, .. } = self.authenticate(bearer, lxm::SESSIONS_RELAY)? else {
            return Err(forbidden("the host tunnel is for accounts"));
        };
        let row = self.require_host(id)?;
        if !row.lease.roster.contains(&xuid) {
            return Err(forbidden("the account has no seat on this host"));
        }
        if row.lease.provider != ProviderKind::Local
            || row.lease.transport != live_proto::hosts::Transport::Tunnel
            || !matches!(row.state, HostState::Joinable | HostState::Playing)
        {
            return Err(conflict("the host tunnel is not available"));
        }

        let provider = self.hosts.provider_for(&row.lease)?;
        let addr = row
            .provider_ref
            .as_deref()
            .and_then(|provider_ref| provider.tunnel_addr(id, provider_ref))
            .ok_or_else(|| unavailable("the host tunnel port is unavailable"))?;
        Ok((xuid, addr))
    }

    /// `POST /v1/admin/hosts` ([`AdminHostStart`]): an admin starts a custom
    /// dedicated host by hand. With `launch` it goes through
    /// [`Self::request_host`], so budget, the host cap, retries and cost rows
    /// all apply. Without it no provider is started: the lease is recorded
    /// `Requested` exactly as a launch would record it — register, report,
    /// the sweep and the inventory treat it like any other — and the boot
    /// credential is answered instead. This is the only route that ever
    /// returns a host token.
    pub async fn admin_start_host(
        &self,
        bearer: Option<&str>,
        request: super::AdminHostStart,
    ) -> ApiResult<(HostLease, Option<String>)> {
        let who = self.authenticate_host_admin(bearer)?;
        self.start_authorized_host(&who, request).await
    }

    pub(crate) fn authenticate_host_admin(&self, bearer: Option<&str>) -> ApiResult<Author> {
        crate::invites::admin(self, bearer, crate::admin::LXM_ADMIN_WRITE)
    }

    pub(crate) async fn start_authorized_host(
        &self,
        who: &Author,
        request: super::AdminHostStart,
    ) -> ApiResult<(HostLease, Option<String>)> {
        self.rate_limit(who, crate::admin::LXM_ADMIN_WRITE)?;
        request.validate()?;
        let Some(provider) = self.host_provider_kind() else {
            return Err(unavailable("no host provider"));
        };
        let launch = request.launch;
        let lease = request.lease(provider);
        if launch {
            return self.request_host(lease).await.map(|lease| (lease, None));
        }
        match self.reserve_host(lease, false).await? {
            Reserved::Fresh { lease, boot, .. } => Ok((lease, Some(boot.token))),
            // A fresh random assignment never collides with an existing one.
            Reserved::Existing(_) => Err(conflict("assignment already has a host")),
        }
    }

    pub fn admin_hosts(&self, bearer: Option<&str>) -> ApiResult<Value> {
        crate::invites::admin(self, bearer, crate::invites::LXM_ADMIN_READ)?;
        let mut rows: Vec<_> = self
            .hosts
            .registry
            .read()
            .unwrap()
            .leases
            .values()
            .cloned()
            .collect();
        rows.sort_by_key(|r| {
            (
                std::cmp::Reverse(r.active()),
                std::cmp::Reverse(r.updated_ms),
            )
        });
        let mut running = Vec::new();
        let mut recent = Vec::new();
        let at = now_ms();
        for row in rows {
            if !row.active() && recent.len() == RECENT {
                break;
            }
            let cost = self
                .hosts
                .costs
                .lease_cost(&row.lease.id, at)
                .unwrap_or_default();
            let mut item = json!(row);
            item["cost_usd"] = json!(cost.cost_usd);
            if row.active() {
                item["uptime_ms"] = json!(cost.ran_ms);
                running.push(item);
            } else {
                recent.push(item);
            }
        }
        Ok(json!({
            "running": running, "recent": recent,
            "month_to_date_usd": self.hosts.costs.month_to_date(at),
            "budget_usd": self.hosts.costs.budget_usd_month(), "position": self.position(),
        }))
    }

    pub async fn admin_stop_host(
        &self,
        bearer: Option<&str>,
        lease_id: &str,
    ) -> ApiResult<HostView> {
        let who = crate::invites::admin(self, bearer, crate::admin::LXM_ADMIN_WRITE)?;
        self.rate_limit(&who, crate::admin::LXM_ADMIN_WRITE)?;
        let _write = self.hosts.writes.lock().await;
        let version = self.hosts.version();
        let row = self.require_host(lease_id)?;
        if row.active() {
            self.stop_host_locked(version, &row, EndReason::Stopped, row.players, now_ms())
                .await?;
        }
        self.require_host(lease_id)
    }

    async fn record_host_end(
        &self,
        version: u64,
        row: &HostView,
        reason: EndReason,
        players: u8,
        at: i64,
    ) -> ApiResult<()> {
        self.host_event(
            version,
            HOST_ENDED,
            HostEnded {
                lease_id: row.lease.id.clone(),
                reason,
                ran_ms: row
                    .started_ms
                    .map_or(0, |start| at.saturating_sub(start).max(0) as u64),
                players_peak: row.players_peak.max(players),
            },
        )
        .await
    }

    async fn record_host_stop(&self, version: u64, lease_id: &str) -> ApiResult<()> {
        self.host_event(
            version,
            HOST_STOPPED,
            HostStopped {
                lease_id: lease_id.into(),
            },
        )
        .await
    }

    async fn stop_host_locked(
        &self,
        mut version: u64,
        row: &HostView,
        reason: EndReason,
        players: u8,
        at: i64,
    ) -> ApiResult<()> {
        if self.held.held() {
            return Err(StoreError::Held.into());
        }
        if let Some(id) = &row.provider_ref {
            let provider = self.hosts.provider_for(&row.lease)?;
            provider
                .stop(&row.lease.id, id)
                .await
                .map_err(|_| unavailable("dedicated host stop failed"))?;
            if self.hosts.costs.stop_confirmed(&row.lease.id) == Some(false) {
                self.record_host_stop(version, &row.lease.id).await?;
                version = self.hosts.version();
            }
        }
        self.record_host_end(version, row, reason, players, at)
            .await
    }

    async fn reap_host(&self, id: &str, at: i64) -> ApiResult<()> {
        let before = self.require_host(id)?;
        let Some((due, _)) = before.deadline() else {
            return Ok(());
        };
        // A slow provider health check on one match must not block another
        // match's registration or a report refreshing its idle deadline.
        let observed_dead = if at < due {
            // A reserved room address exists before the provider is called.
            if before.state == HostState::Requested {
                return Ok(());
            }
            let Some(reference) = &before.provider_ref else {
                return Ok(());
            };
            let provider = self.hosts.provider_for(&before.lease)?;
            if provider
                .alive(reference)
                .await
                .map_err(|_| unavailable("host liveness check failed"))?
            {
                return Ok(());
            }
            true
        } else {
            false
        };
        let _write = self.hosts.writes.lock().await;
        let version = self.hosts.version();
        let row = self.require_host(id)?;
        let Some((due, reason)) = row.deadline() else {
            return Ok(());
        };
        if at >= due {
            self.stop_host_locked(version, &row, reason, row.players, at)
                .await?;
        } else if observed_dead && row.provider_ref == before.provider_ref {
            self.record_host_end(version, &row, EndReason::Crashed, row.players, at)
                .await?;
        }
        Ok(())
    }

    pub fn host_sweep_due_ms(&self) -> Option<i64> {
        if self.held.held() {
            return None;
        }
        let registry = self.hosts.registry.read().unwrap();
        let cleanup = registry
            .cleanup
            .first_key_value()
            .and_then(|(_, id)| registry.leases.get(id))
            .map(|r| r.updated_ms.saturating_add(EXIT_GRACE_MS));
        let unrecorded = registry
            .launching
            .values()
            .any(Option::is_some)
            .then(now_ms);
        let last_probe = self.hosts.last_probe_ms.load(Ordering::Relaxed);
        let next_probe = registry
            .leases
            .values()
            .filter(|r| r.active())
            .filter_map(|r| r.started_ms)
            .min()
            .map(|start| start.max(last_probe).saturating_add(PROBE_MS));
        registry
            .leases
            .values()
            .filter_map(|r| r.deadline().map(|(at, _)| at))
            .chain(cleanup)
            .chain(unrecorded)
            .chain(next_probe)
            .min()
    }

    pub async fn sweep_hosts(&self) {
        self.sweep_hosts_at(now_ms()).await;
        self.refresh_host_pools().await;
    }

    pub(super) async fn sweep_hosts_at(&self, at: i64) {
        if self.held.held() {
            return;
        }
        self.hosts.last_probe_ms.store(at, Ordering::Relaxed);
        let ids: Vec<_> = self
            .hosts
            .registry
            .read()
            .unwrap()
            .leases
            .values()
            .filter(|r| r.active())
            .map(|r| r.lease.id.clone())
            .collect();
        for id in ids {
            if let Err(e) = self.reap_host(&id, at).await {
                eprintln!("live: reaping host {id}: {e}");
            }
        }
        if let Err(e) = self.stop_unrecorded_hosts().await {
            eprintln!("live: stopping an unrecorded host: {e}");
        }
        if let Err(e) = self.clean_up_ended_hosts(at).await {
            eprintln!("live: retiring ended hosts: {e}");
        }
    }

    async fn stop_unrecorded_hosts(&self) -> ApiResult<()> {
        let _write = self.hosts.writes.lock().await;
        let refs: Vec<_> = self
            .hosts
            .registry
            .read()
            .unwrap()
            .launching
            .iter()
            .filter_map(|(lease_id, id)| Some((lease_id.clone(), id.clone()?)))
            .collect();
        for (lease_id, id) in refs {
            let row = self.require_host(&lease_id)?;
            self.hosts
                .provider_for(&row.lease)?
                .stop(&lease_id, &id)
                .await
                .map_err(|_| unavailable("could not stop an unrecorded host"))?;
            self.hosts
                .registry
                .write()
                .unwrap()
                .launching
                .remove(&lease_id);
        }
        Ok(())
    }

    // Advance only after a provider confirms exit or completes stop. A
    // restart replays the remaining terminal events, and an old PID alone
    // is never enough authority to kill a local process.
    async fn clean_up_ended_hosts(&self, at: i64) -> ApiResult<()> {
        let _work = self.hosts.cleanup_work.lock().await;
        for _ in 0..32 {
            let next = {
                let registry = self.hosts.registry.read().unwrap();
                registry
                    .cleanup
                    .first_key_value()
                    .and_then(|(position, id)| {
                        registry.leases.get(id).map(|row| (*position, row.clone()))
                    })
            };
            let Some((position, row)) = next else { break };
            if row.updated_ms.saturating_add(EXIT_GRACE_MS) > at {
                break;
            }
            let id = row
                .provider_ref
                .as_deref()
                .expect("cleanup has a provider reference");
            match self.hosts.costs.stop_confirmed(&row.lease.id) {
                Some(false) => {
                    // Stop also fences a delayed start for this known room.
                    // An alive check cannot tell whether a start is in flight.
                    self.hosts
                        .provider_for(&row.lease)?
                        .stop(&row.lease.id, id)
                        .await
                        .map_err(|_| unavailable("ended host cleanup failed"))?;
                    let _write = self.hosts.writes.lock().await;
                    self.record_host_stop(self.hosts.version(), &row.lease.id)
                        .await?;
                }
                Some(true) => {}
                None => {
                    let provider = self.hosts.provider_for(&row.lease)?;
                    if provider
                        .alive(id)
                        .await
                        .map_err(|_| unavailable("host exit check failed"))?
                    {
                        provider
                            .stop(&row.lease.id, id)
                            .await
                            .map_err(|_| unavailable("ended host cleanup failed"))?;
                    }
                }
            }
            self.store.save_checkpoint(CLEANUP_CURSOR, position).await?;
            let mut registry = self.hosts.registry.write().unwrap();
            registry.cleaned_through = position;
            registry.cleanup.remove(&position);
        }
        Ok(())
    }

    pub fn sweep_due_with_hosts(&self) -> Option<i64> {
        let host_due = self
            .host_sweep_due_ms()
            .map(|ms| ms.saturating_add(999).div_euclid(1000));
        self.sweep_due().into_iter().chain(host_due).min()
    }

    pub async fn sweep_with_hosts(&self) {
        self.sweep().await;
        self.sweep_hosts().await;
    }
}

fn elapsed(lease: &HostLease, at: i64) -> u64 {
    at.saturating_sub(lease.created_ms).max(0) as u64
}

fn start_deadline(lease: &HostLease) -> i64 {
    lease.created_ms.saturating_add(BOOT_MS).min(lease.deadline_ms())
}
