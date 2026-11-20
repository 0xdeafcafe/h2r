//! Game invitations on recipient mailbox streams, independent of sign-up invites.
use crate::Xuid;
use serde::{Deserialize, Serialize};
pub const SENT: &str = "GameInviteSent";
pub const ANSWERED: &str = "GameInviteAnswered";
pub fn is_kind(k: &str) -> bool {
    matches!(k, SENT | ANSWERED)
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
