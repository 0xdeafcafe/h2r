//! Game invitations and friends' text messages on recipient mailbox streams,
//! independent of sign-up invites.
use crate::Xuid;
use serde::{Deserialize, Serialize};
pub const SENT: &str = "GameInviteSent";
pub const ANSWERED: &str = "GameInviteAnswered";
/// A friend's text message to the mailbox owner.
pub const TEXT_SENT: &str = "MessageSent";
/// The mailbox owner deleting one text message.
pub const TEXT_DELETED: &str = "MessageDeleted";
/// The longest message text, in characters.
pub const TEXT_MAX: usize = 512;
/// Text messages kept per recipient until they delete some.
pub const TEXT_BOX_MAX: usize = 100;
pub fn is_kind(k: &str) -> bool {
    matches!(k, SENT | ANSWERED | TEXT_SENT | TEXT_DELETED)
}
pub fn stream(x: Xuid) -> String {
    format!("mailbox-{x}")
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Accept,
    Decline,
    Remove,
    Block,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sent {
    pub from: Xuid,
    pub session_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answered {
    pub by: Xuid,
    pub session_id: String,
    pub invite_id: u64,
    pub answer: Answer,
}
/// `MessageSent`: the sender, the signing account, to the stream's owner.
/// The sent time is the signed event's own time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextSent {
    pub from: Xuid,
    pub text: String,
}
/// `MessageDeleted`: the owner removes the message sent at `message_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextDeleted {
    pub by: Xuid,
    pub message_id: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextMessage {
    /// The sent event's log position.
    pub id: u64,
    pub from: Xuid,
    pub to: Xuid,
    pub gamertag: String,
    pub text: String,
    pub sent_at: i64,
}
/// `GET /v1/accounts/by-xuid/{xuid}/mailbox`: the pending invitations and
/// the text messages, each newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mailbox {
    pub items: Vec<Invite>,
    #[serde(default)]
    pub messages: Vec<TextMessage>,
    pub next: Option<String>,
    pub position: u64,
}
/// Whether `text` may be sent: 1..=[`TEXT_MAX`] characters, not blank, no
/// control characters but newline and tab.
pub fn valid_text(text: &str) -> bool {
    let n = text.chars().count();
    (1..=TEXT_MAX).contains(&n)
        && !text.trim().is_empty()
        && !text.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub id: u64,
    pub from: Xuid,
    pub to: Xuid,
    pub gamertag: String,
    pub session_id: String,
    pub sent_at: i64,
    pub answer: Option<Answer>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn answers_round_trip_and_payloads_reject_unknown_fields() {
        for answer in [
            Answer::Accept,
            Answer::Decline,
            Answer::Remove,
            Answer::Block,
        ] {
            let body = Answered {
                by: Xuid(2),
                session_id: "8000000000000001".into(),
                invite_id: 7,
                answer,
            };
            let encoded = serde_json::to_value(&body).unwrap();
            let parsed: Answered = serde_json::from_value(encoded).unwrap();
            assert_eq!(parsed.answer, answer);
        }
        assert!(
            serde_json::from_value::<Sent>(
                json!({"from":Xuid(1),"session_id":"8000000000000001","key":"not a public field"})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<Answered>(json!({"by":Xuid(2),"session_id":"8000000000000001","invite_id":7,"answer":"unknown"})).is_err());
        assert_eq!(stream(Xuid(2)), "mailbox-0000000000000002");
    }
}
