//! Short-lived viewport notices and the latest player-move refusal.

use std::time::{Duration, Instant};

use agent_proto::RpcError;
use serde_json::Value;

use super::{App, Play, presence::PlayerMove};

const TOAST_LIFETIME: Duration = Duration::from_secs(4);

struct Toast {
    text: String,
    problem: bool,
    expires: Instant,
}

/// Identifies the document and local player to which move feedback belongs.
/// A View-mode refusal also has a scope, even though that mode cannot send a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveContext {
    pairing: u64,
    paired: bool,
    world: u64,
    epoch: Option<u64>,
    checksum: Option<u32>,
    sample_epoch: Option<u64>,
    player: Option<(u16, u32)>,
    editing: bool,
}

struct MoveRefusal {
    scope: MoveContext,
    reason: String,
    expires: Instant,
}

#[derive(Default)]
pub(super) struct Feedback {
    notices: Vec<Toast>,
    refused_move: Option<MoveRefusal>,
    next_ticket: u64,
    sent_move: Option<(u64, MoveContext, PlayerMove)>,
}

impl Feedback {
    pub(super) fn toast(&mut self, text: String, problem: bool, now: Instant) {
        self.notices.retain(|notice| notice.expires > now);
        self.notices.push(Toast { text, problem, expires: now + TOAST_LIFETIME });
    }

    pub(super) fn problem(&mut self, app: &mut App, why: String) {
        self.toast(why.clone(), true, Instant::now());
        app.error(why);
    }

    pub(super) fn sent_move(&mut self, scope: MoveContext, request: &PlayerMove) -> u64 {
        self.next_ticket = self.next_ticket.wrapping_add(1);
        self.sent_move = Some((self.next_ticket, scope, request.clone()));
        self.next_ticket
    }

    pub(super) fn clear_moves(&mut self) {
        self.sent_move = None;
        self.refused_move = None;
    }
}

impl Play {
    pub(crate) fn toasts(&self, now: Instant) -> impl Iterator<Item = (&str, bool)> {
        self.feedback.notices.iter().rev()
            .filter(move |notice| now < notice.expires)
            .map(|notice| (notice.text.as_str(), notice.problem))
    }

    pub(super) fn move_context(&self, app: &App) -> MoveContext {
        let sample = app.pairing.shown.as_ref();
        let player = sample.and_then(|s| {
            s.players.iter().filter(|p| p.local_user.is_some() && p.unit.is_some())
                .min_by_key(|p| (p.local_user, p.index))
        });
        MoveContext {
            pairing: self.pairing_id,
            paired: self.is_paired(),
            world: app.session.world_rev(),
            epoch: self.edits.epoch,
            checksum: self.edits.checksum,
            sample_epoch: sample.map(|s| s.epoch),
            player: player.and_then(|p| p.unit.map(|unit| (p.index, unit))),
            editing: self.live_edits,
        }
    }

    pub(super) fn expire_move_feedback(&mut self, app: &App, now: Instant) {
        if let Some(issue) = &self.feedback.refused_move
            && (now >= issue.expires || issue.scope != self.move_context(app))
        {
            self.feedback.refused_move = None;
        }
    }

    pub(crate) fn refused_move_at(&self, now: Instant) -> Option<&str> {
        self.feedback.refused_move.as_ref()
            .filter(|issue| now < issue.expires)
            .map(|issue| issue.reason.as_str())
    }

    pub(crate) fn game_refusal(&mut self, app: &mut App, message: impl Into<String>) {
        let reason = message.into();
        let now = Instant::now();
        self.feedback.sent_move = None;
        self.feedback.refused_move = Some(MoveRefusal {
            scope: self.move_context(app),
            reason: reason.clone(),
            expires: now + TOAST_LIFETIME,
        });
        self.feedback.toast(reason.clone(), true, now);
        app.error(reason);
    }

    pub(super) fn move_answer(
        &mut self,
        app: &mut App,
        ticket: u64,
        result: Result<Value, RpcError>,
    ) {
        if self.feedback.sent_move.as_ref().is_none_or(|(sent, _, _)| *sent != ticket) {
            return;
        }
        let (_, scope, request) = self.feedback.sent_move.take().unwrap();
        if scope != self.move_context(app)
            || !app.pairing.mirror.as_ref().is_some_and(|stream| request.check_latest(stream).is_ok())
        {
            return;
        }
        match result {
            Ok(_) => {
                self.feedback.refused_move = None;
                app.info("moved the player");
            }
            Err(e) => {
                let why = e.data.as_ref().and_then(|d| d["why"].as_str()).unwrap_or(&e.message);
                self.game_refusal(app, why);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::play::link::Purpose;
    use crate::play::testing::{Temp, fake};
    use crate::play::tests::{editor as open_editor, finder_for, paired};
    use agent::Access;
    use agent_proto::sim::{self, SessionKind};

    #[test]
    fn refused_teleport_raises_one_toast_for_four_seconds() {
        let temp = Temp::new("teleport-toast");
        let game = fake::Game::start("teleport-toast", "example", SessionKind::Local);
        game.agent.add_game_method(sim::method::STATE_SET, Access::Act, |_, _| {
            let mut error = RpcError::new(sim::REFUSED, "player move refused");
            error.data = Some(serde_json::json!({"why": "the fixture player is seated"}));
            Err(error)
        });
        let mut app = open_editor(&temp);
        let mut play = Play::new();
        fake::pair_for_view(&mut play, &mut app, &game);
        play.request(crate::play::pair::Request::Edit(true));
        fake::until(&mut play, &mut app, "local player in Edit", |_, a| {
            crate::play::presence::PlayerMove::capture(a, [3., 1., 4.], false).is_ok()
        });
        play.teleport(&mut app, [3.0, 1.0, 4.0]);
        fake::until(&mut play, &mut app, "refused move", |p, _| {
            p.toasts(Instant::now()).any(|(text, _)| text.contains("fixture player is seated"))
        });
        let expiry = play.feedback.notices.last().unwrap().expires;
        let mut visible = play.toasts(expiry - Duration::from_millis(1));
        let (text, problem) = visible.next().unwrap();
        assert!(problem && text.contains("fixture player is seated"));
        assert!(visible.next().is_none());
        drop(visible);
        assert!(play.toasts(expiry).next().is_none());
        assert_eq!(play.refused_move_at(expiry), None);

        let mut api = crate::api::Api::new();
        let status = api.call(&mut app, studio_proto::method::PAIR_STATUS, Value::Null).unwrap();
        let target = serde_json::json!({"panel": "pending"});
        assert_eq!(status["link"]["issues"], serde_json::json!([{
            "kind": "refused", "text": "the fixture player is seated", "target": target
        }]));
        api.call(&mut app, studio_proto::method::NAV_GO, target).unwrap();
        assert!(app.pairing.open_pending);
        assert!(app.pairing.pending.is_empty());
        assert_eq!(app.log().iter().filter(|l| l.status.text() == "the fixture player is seated").count(), 1);

        play.feedback.refused_move.as_mut().unwrap().expires = Instant::now();
        play.publish(&mut app);
        let status = api.call(&mut app, studio_proto::method::PAIR_STATUS, Value::Null).unwrap();
        assert_eq!(status["link"]["issues"], serde_json::json!([]));
    }

    #[test]
    fn a_successful_move_clears_the_active_issue_and_keeps_the_earlier_notice() {
        let (_temp, _game, mut app, mut play) = paired("move-feedback", "example", SessionKind::Local);
        play.request(crate::play::pair::Request::Edit(true));
        fake::until(&mut play, &mut app, "movable player", |_, a| {
            PlayerMove::capture(a, [1., 2., 3.], false).is_ok()
        });
        play.game_refusal(&mut app, "fixture move refused");
        play.teleport(&mut app, [2., 3., 4.]);
        fake::until(&mut play, &mut app, "successful move", |_, a| {
            a.log().iter().any(|line| line.status.text() == "moved the player")
        });
        assert!(play.toasts(Instant::now()).any(|(text, _)| text == "fixture move refused"));
        assert!(play.refused_move_at(Instant::now()).is_none());
        assert!(play.link_view(&app).issues.iter().all(|issue| issue.text != "fixture move refused"));

        play.game_refusal(&mut app, "second refusal");
        play.unpair(&mut app);
        assert!(play.refused_move_at(Instant::now()).is_none());
        assert!(play.toasts(Instant::now()).any(|(text, _)| text == "second refusal"));
    }

    #[test]
    fn stale_action_answers_never_raise_a_notice() {
        let mut app = App::new();
        let mut play = Play::new();
        for (purpose, method) in [
            (Purpose::Object(77), sim::method::OBJECT_DELETE),
            (Purpose::Teleport(78), sim::method::STATE_SET),
            (Purpose::Spawn(Instant::now()), sim::method::STATE_SET),
        ] {
            play.on_answer(&mut app, purpose, method, Err(RpcError::new(sim::REFUSED, "stale answer")));
        }
        assert!(play.toasts(Instant::now()).next().is_none());
        assert!(app.log().iter().all(|line| !line.status.text().contains("stale answer")));
    }

    #[test]
    fn link_loss_and_verified_return_each_raise_one_toast() {
        let (_temp, game, mut app, mut play) = paired("link-toasts", "example", SessionKind::Local);
        play.finder = finder_for(&[&game]);
        fake::until(&mut play, &mut app, "listed game", |p, _| !p.finder.running().is_empty());
        play.on_event(&mut app, crate::play::link::Event::Lost("fixture drop".into()));
        play.on_event(&mut app, crate::play::link::Event::Lost("retry failed".into()));
        assert_eq!(play.toasts(Instant::now()).count(), 1);
        assert!(play.toasts(Instant::now()).any(|(text, problem)| problem && text.contains("fixture drop")));
        play.reconnect(&mut app, Instant::now() + Duration::from_secs(1));
        fake::until(&mut play, &mut app, "verified return", |p, _| p.is_paired());
        assert!(play.toasts(Instant::now()).any(|(text, problem)| !problem && text == "Game link restored"));
        assert_eq!(app.log().iter().filter(|l| l.status.text() == "Game link restored").count(), 1);
    }
}
