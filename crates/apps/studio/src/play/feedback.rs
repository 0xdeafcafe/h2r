//! Short-lived viewport notices from game actions and the control connection.

use std::time::{Duration, Instant};

use agent_proto::RpcError;
use serde_json::Value;

use super::{App, Play, Purpose};

const TOAST_LIFETIME: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Notice {
    Info,
    Problem,
    RefusedMove,
}

struct Toast {
    text: String,
    kind: Notice,
    expires: Instant,
}

#[derive(Default)]
pub(super) struct Feedback {
    notices: Vec<Toast>,
}

impl Feedback {
    pub(super) fn toast(&mut self, text: String, problem: bool, now: Instant) {
        let kind = if problem { Notice::Problem } else { Notice::Info };
        self.push(text, kind, now);
    }

    pub(super) fn problem(&mut self, app: &mut App, why: String) {
        self.push(why.clone(), Notice::Problem, Instant::now());
        app.error(why);
    }

    fn push(&mut self, text: String, kind: Notice, now: Instant) {
        self.notices.retain(|notice| notice.expires > now);
        self.notices.push(Toast {
            text,
            kind,
            expires: now + TOAST_LIFETIME,
        });
    }

    /// Polls can fail repeatedly while a map loads. Surface action replies here;
    /// their individual handlers still update the document and log the outcome.
    pub(super) fn answer(
        &mut self,
        purpose: &Purpose,
        method: &str,
        result: &Result<Value, RpcError>,
    ) {
        if let Err(e) = result
            && !matches!(
                purpose,
                Purpose::Capabilities | Purpose::LiveStatus | Purpose::Map
                    | Purpose::Journal(_) | Purpose::Indicator | Purpose::Marks | Purpose::Teleport
            )
        {
            let why = e.data.as_ref().and_then(|d| d["why"].as_str()).unwrap_or(&e.message);
            self.toast(format!("{method}: {why}"), true, Instant::now());
        }
    }
}

impl Play {
    pub(crate) fn toasts(&self, now: Instant) -> impl Iterator<Item = (&str, bool)> {
        self.feedback.notices.iter().rev()
            .filter(move |notice| now < notice.expires)
            .map(|notice| (notice.text.as_str(), notice.kind != Notice::Info))
    }

    /// Pending and the link report the latest refused player move while its toast is visible.
    pub(crate) fn refused_move_at(&self, now: Instant) -> Option<&str> {
        self.feedback.notices.iter().rev()
            .find(|notice| notice.kind == Notice::RefusedMove && now < notice.expires)
            .map(|notice| notice.text.as_str())
    }

    pub(crate) fn game_refusal(&mut self, app: &mut App, message: impl Into<String>) {
        let text = message.into();
        self.feedback.push(text.clone(), Notice::RefusedMove, Instant::now());
        app.error(text);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
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

        play.feedback.notices.last_mut().unwrap().expires = Instant::now();
        play.publish(&mut app);
        let status = api.call(&mut app, studio_proto::method::PAIR_STATUS, Value::Null).unwrap();
        assert_eq!(status["link"]["issues"], serde_json::json!([]));
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
