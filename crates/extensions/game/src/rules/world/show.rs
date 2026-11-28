//! The show channel's messages and state (docs/game/rules-world-api.md,
//! "The show channel"): a script's `message(to, text)` on the host, the
//! cues a distributed client receives, and the script's show state that
//! rides the channel's `Show` record.
//!
//! On the authority (the machine `GAME_ENGINE_AUTHORITATIVE` answers true
//! for; `Runtime::pre_tick` holds the messages until it has), [`publish`]
//! turns each message the VM made since the last boundary into a line
//! shown to this machine's local users in its audience (the HUD kit's
//! notification insert) and a [`PendingCue`] for the actor channel, which
//! the capture (`crate::actor_net`) drains once a tick; [`offer_state`]
//! gives the capture the script's show state the same way. On a
//! `Viewer`, [`present`] decodes each received cue against the running
//! script's blob, checks every field, and shows the line to the local
//! users in its audience, and [`receive`] holds the newest state that
//! passes every check and draws the HUDs it feeds.

use std::sync::{Arc, Mutex, MutexGuard};

use ::rules::show::{self, Audience, Cue, HoleValue, Message, State, To};
use ::rules::types::{StartLine, Type};
use ::rules::vm::Arg;
use ::rules::{Ir, caps};
use guest_layout::GuestMemory;
use guest_layout::entry::Cx;

use crate::actor_net::client::ReceivedCue;
use crate::rules::census::{self, Table};

use super::hud;
use super::zones::Zones;

/// A cue waiting for the actor channel: the tick it was made, its
/// per-tick sequence and the cue itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCue {
    pub tick: u32,
    pub sequence: u8,
    pub cue: Cue,
}

/// A message the VM made, waiting for the tick boundary (the runtime's
/// `Call::Message`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub to: To,
    pub template: String,
    pub values: Vec<Arg>,
}

/// The message of a `message*` call, for the boundary: its audience,
/// template and hole values. A call through `none` shows nothing.
pub fn outgoing(call: &::rules::vm::Call) -> Option<Outgoing> {
    let own = call.entry.args.len();
    let to = match call.entry.id {
        caps::id::MESSAGE => match call.args.first() {
            Some(Arg::Player(Some(p))) => To::Player(*p),
            _ => return None,
        },
        caps::id::MESSAGE_TEAM => match call.args.first() {
            Some(Arg::Team(Some(t))) => To::Team(*t),
            _ => return None,
        },
        caps::id::MESSAGE_ALL => To::Everyone,
        _ => return None,
    };
    let Some(Arg::Text(template)) = call.args.get(own - 1) else {
        return None;
    };
    Some(Outgoing {
        to,
        template: template.clone(),
        values: call.args[own..].to_vec(),
    })
}

struct Channel {
    cues: Vec<PendingCue>,
    tick: Option<u32>,
    next: u8,
}

static CHANNEL: Mutex<Channel> = Mutex::new(Channel {
    cues: Vec::new(),
    tick: None,
    next: 0,
});

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Queues the cue for the channel with its tick's next sequence number.
pub(super) fn push(tick: u32, cue: Cue) {
    let mut c = lock(&CHANNEL);
    if c.tick != Some(tick) {
        c.tick = Some(tick);
        c.next = 0;
    }
    let sequence = c.next;
    c.next = c.next.wrapping_add(1);
    c.cues.push(PendingCue {
        tick,
        sequence,
        cue,
    });
    // A machine with no consumer (a solo game, a synchronous peer) drops
    // the oldest past the bound.
    let over = c.cues.len().saturating_sub(show::MAX_PENDING);
    if over > 0 {
        c.cues.drain(..over);
    }
}

/// The authority speaks the same clip it sends its viewers. The type
/// table supplies the id on both ends.
pub fn start_clip(tick: u32, ty: Type) {
    if ty.row().start == StartLine::Ours {
        crate::medal_announcer::announce_clip(ty.name());
        push(tick, Cue::StartClip(ty));
    }
}

/// The authority's walls cue at `tick`: the whole list of the areas the
/// script shows (`::rules::show::Cue::Walls`).
pub fn walls(tick: u32, shown: Vec<(u32, Option<u8>)>) {
    push(tick, Cue::Walls(shown));
}

/// The authority's pickers' offer cue at `tick`: one picker's offer,
/// whole (`::rules::show::Cue::Pick`; an empty one takes the picker's
/// offer away on the clients, where a player picks and the pick goes
/// back up the channel, `super::pick`).
pub fn pick_offer(tick: u32, offer: ::rules::vm::PickOffer) {
    push(tick, Cue::Pick(offer));
}

/// The cues waiting for the channel.
pub fn take_cues() -> Vec<PendingCue> {
    std::mem::take(&mut lock(&CHANNEL).cues)
}

/// The waiting cues as the actor channel's `Cue` records, drained by its
/// capture once a tick. A cue that does not encode is dropped and logged.
pub fn take_records() -> Vec<::actor_net::Record> {
    take_cues()
        .into_iter()
        .filter_map(|pending| match pending.cue.encode() {
            Ok(bytes) => Some(::actor_net::Record::Cue {
                tick: pending.tick,
                sequence: pending.sequence,
                kind: pending.cue.kind(),
                bytes,
            }),
            Err(error) => {
                eprintln!("rules: show cue at tick {}: {error}", pending.tick);
                None
            }
        })
        .collect()
}

/// Forgets the waiting cues and the state offered for the channel: a new
/// match or a snapshot restore starts it empty.
pub fn clear_cues() {
    *lock(&WALLS_SEEN) = None;
    crate::forge_glue::markers_world::publish_walls(Vec::new());
    *lock(&CHANNEL) = Channel {
        cues: Vec::new(),
        tick: None,
        next: 0,
    };
    *lock(&OUTGOING) = None;
}

/// The script's show state (`RulesExt::show`) offered for the actor
/// channel's `Show` record, encoded: the runtime's `post_tick` offers it
/// once a tick from the authority that runs the script, and forgets it
/// when no script of ours offers one.
static OUTGOING: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// Offers the state's bytes for this tick's capture.
pub fn offer_state(bytes: Vec<u8>) {
    *lock(&OUTGOING) = Some(bytes);
}

/// Forgets what was offered: no script of ours offers a state, so the
/// channel retracts the last one with an empty record.
pub fn forget_state() {
    *lock(&OUTGOING) = None;
}

/// The offered state's bytes, taken by the capture once a tick
/// (`crate::actor_net::show_state`).
pub fn take_state() -> Option<Vec<u8>> {
    std::mem::take(&mut lock(&OUTGOING))
}

/// The show state a client holds (the newest `Show` record that passed
/// every check), as its bytes. The digest's show hash is of these.
static HELD: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// The held state's bytes; none when no state is held.
pub fn held_bytes() -> Option<Vec<u8>> {
    lock(&HELD).clone()
}

/// Forgets the held state and hides the HUDs it fed: a new match, a
/// machine that left its session or a lost game starts it empty.
pub fn clear_held() {
    let held = lock(&HELD).take();
    if held.is_some() {
        hud::clear();
    }
}

/// The running rules script's checked blob, from the variant record's
/// blob (compiled in or in the game-variant store), cached by the
/// record's hash prefix. `None` for a game type no script of ours runs.
fn script_ir(m: &dyn GuestMemory) -> Option<Arc<Ir>> {
    type Cached = ([u8; 8], Arc<Ir>);
    static CACHE: Mutex<Option<Cached>> = Mutex::new(None);
    let block = super::super::guest::block(m).ok().flatten()?;
    let script = variants::ext::Ext::in_effect(&block).script;
    if script.mode != variants::ext::Mode::Rules {
        return None;
    }
    {
        let cache = lock(&CACHE);
        if let Some((prefix, ir)) = &*cache
            && *prefix == script.prefix
        {
            return Some(Arc::clone(ir));
        }
    }
    let blob = super::super::feature::local_blob(script.prefix)?;
    let (ir, _) = ::rules::verify::load(&blob).ok()?;
    let ir = Arc::new(ir);
    *lock(&CACHE) = Some((script.prefix, Arc::clone(&ir)));
    Some(ir)
}

/// The `Show` records a client received this tick, newest last: the
/// newest one that passes every check is the state held
/// (docs/game/rules-world-limits.md, "The show channel on a client"), an
/// empty record retracts it, and the HUDs the script feeds are drawn
/// from what is held. A record that fails a check is dropped whole and
/// logged.
pub async fn receive(cx: &Cx, shows: &[Vec<u8>]) {
    for bytes in shows {
        if bytes.is_empty() {
            clear_held();
            continue;
        }
        if let Err(why) = receive_one(cx, bytes) {
            eprintln!("rules: show state dropped: {why}");
        }
    }
    publish_show(cx).await;
}

/// Checks one record against the running script's blob and holds it: the
/// state decodes, every widget and waypoint it sets is one the blob
/// declares, and every text index is in the blob's string table.
fn receive_one(cx: &Cx, bytes: &[u8]) -> Result<(), String> {
    let ir = cx
        .read(script_ir)
        .ok_or_else(|| "the running script's blob is not on this machine".to_string())?;
    checked(&ir, bytes)?;
    *lock(&HELD) = Some(bytes.to_vec());
    Ok(())
}

/// What a record's bytes must answer to the blob they name: the state
/// decodes, every widget and waypoint it sets is one the blob declares,
/// and every text index is in the blob's string table.
fn checked(ir: &Ir, bytes: &[u8]) -> Result<State, String> {
    let state = State::decode(bytes).map_err(|e| e.to_string())?;
    for (i, set) in state.widgets.iter().enumerate() {
        let Some(w) = set else { continue };
        if i >= ir.widgets.len() {
            return Err(format!("widget {i} is not one the script declares"));
        }
        if let Some(text) = w.text
            && usize::from(text) >= ir.strings.len()
        {
            return Err(format!("text {text} is not in the script's string table"));
        }
    }
    for (i, set) in state.waypoints.iter().enumerate() {
        if set.is_some() && i >= ir.waypoints.len() {
            return Err(format!("waypoint {i} is not one the script declares"));
        }
    }
    Ok(state)
}

/// Draws the held state's HUDs for this machine's local users, on a
/// machine that runs no VM: the zones are read from this machine's own
/// copy of the map, which is the one the host's script ran over
/// (`super::walls::client`). With nothing held, the HUDs a script fed
/// are hidden.
async fn publish_show(cx: &Cx) {
    let Some(bytes) = held_bytes() else {
        return;
    };
    let Some(ir) = cx.read(script_ir) else {
        return;
    };
    let Ok(state) = State::decode(&bytes) else {
        return;
    };
    let fed = cx.read(|m| {
        let things = census::things(m).unwrap_or_default();
        let zones = Zones::read(m, &Table::of_census(&things));
        hud::publish(m, &ir, &state, &zones)
    });
    if !fed {
        hud::clear();
    }
}

/// One listed player: its roster slot and team, its name and, when it is
/// this machine's, its local user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Roster {
    pub(crate) slot: u8,
    pub(crate) team: Option<u8>,
    pub(crate) name: String,
    pub(crate) user: Option<u32>,
}

/// The listed players: slot, team, name and local user reads.
pub(crate) fn roster(m: &dyn GuestMemory) -> Vec<Roster> {
    use guest_layout::view::players;
    let Ok(Some(array)) = players::array(m) else {
        return Vec::new();
    };
    let Ok(live) = array.live(m, 16) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (handle, p) in live {
        if !p.listed(m).unwrap_or(false) {
            continue;
        }
        out.push(Roster {
            slot: handle.index() as u8,
            team: p.team(m).ok().filter(|t| *t >= 0).map(|t| t as u8),
            name: p.name(m).unwrap_or_default(),
            user: p.local_user(m).ok().filter(|u| *u >= 0).map(|u| u as u32),
        });
    }
    out
}

/// The audience of `to` over the roster: the player's bit, the bits of the
/// team's seated players, or every slot.
fn audience_of(roster: &[Roster], to: &To) -> Audience {
    match to {
        To::Player(slot) => show::player(*slot),
        To::Team(team) => roster
            .iter()
            .filter(|p| p.team == Some(*team))
            .fold(0, |mask, p| mask | show::player(p.slot)),
        To::Everyone => show::EVERYONE,
    }
}

/// The audience of `to` over the game's roster ([`audience_of`]).
pub(super) fn audience(m: &dyn GuestMemory, to: &To) -> Audience {
    audience_of(&roster(m), to)
}

/// The slots of this machine's local players.
pub(super) fn local_slots(m: &dyn GuestMemory) -> Vec<u8> {
    roster(m)
        .into_iter()
        .filter(|p| p.user.is_some())
        .map(|p| p.slot)
        .collect()
}

/// This machine's local players: roster slot and controller port.
pub(super) fn local_seats(m: &dyn GuestMemory) -> Vec<(u8, u32)> {
    roster(m)
        .into_iter()
        .filter_map(|p| p.user.map(|u| (p.slot, u)))
        .collect()
}

/// The name a player hole shows: the roster slot's name, or `player N`
/// when the slot is not seated.
fn named(roster: &[Roster]) -> impl Fn(u8) -> String + '_ {
    move |slot| {
        roster
            .iter()
            .find(|p| p.slot == slot)
            .map_or_else(|| format!("player {slot}"), |p| p.name.clone())
    }
}

/// A hole value's word in a shown line, as the fake renders it
/// (`rules::fake`): a player by name, a team as `team N`, a time as
/// `m:ss`, an enum by its name.
fn arg_word(value: &Arg, hole: Option<caps::Hole>, name: &dyn Fn(u8) -> String) -> String {
    match (value, hole) {
        (Arg::Int(n), Some(caps::Hole::Time)) => caps::show_time(*n),
        (Arg::Int(n), _) => n.to_string(),
        (Arg::Player(Some(p)), _) => name(*p),
        (Arg::Team(Some(t)), _) => format!("team {t}"),
        (Arg::Player(None) | Arg::Team(None), _) => "none".into(),
        (Arg::Text(s), _) => s.clone(),
        (other, _) => format!("{other:?}"),
    }
}

/// A message's line: its template filled with its values' words.
fn arg_line(template: &str, values: &[Arg], name: &dyn Fn(u8) -> String) -> String {
    let holes = caps::holes(template).unwrap_or_default();
    let shown: Vec<String> = values
        .iter()
        .zip(
            holes
                .iter()
                .copied()
                .map(Some)
                .chain(std::iter::repeat(None)),
        )
        .map(|(v, h)| arg_word(v, h, name))
        .collect();
    caps::fill(template, &shown)
}

/// The channel cue of a message: the template and each enum value as an
/// index of the blob's string table, so a record names only what the
/// verified blob holds.
fn cue_of(ir: &Ir, audience: Audience, template: &str, values: &[Arg]) -> Option<Cue> {
    let index = |s: &str| {
        ir.strings
            .iter()
            .position(|t| t == s)
            .and_then(|i| u16::try_from(i).ok())
    };
    let holes = caps::holes(template)?;
    let text = index(template)?;
    let mut out = Vec::with_capacity(values.len());
    for (value, hole) in values.iter().zip(
        holes
            .iter()
            .copied()
            .map(Some)
            .chain(std::iter::repeat(None)),
    ) {
        out.push(match (value, hole) {
            (Arg::Player(p), _) => HoleValue::Player(*p),
            (Arg::Team(t), _) => HoleValue::Team(*t),
            (Arg::Int(n), Some(caps::Hole::Time)) => HoleValue::Time(*n),
            (Arg::Int(n), _) => HoleValue::Int(*n),
            (Arg::Text(name), _) => HoleValue::Enum(index(name)?),
            _ => return None,
        });
    }
    Some(Cue::Message(Message {
        audience,
        text,
        values: out,
    }))
}

/// The messages the VM made since the last boundary, at `tick`: each
/// shown to this machine's local users in its audience and queued for the
/// channel. A message with an empty audience (a team with no players) is
/// nothing.
pub fn publish(m: &dyn GuestMemory, ir: &Ir, tick: u32, messages: Vec<Outgoing>) {
    if messages.is_empty() {
        return;
    }
    let roster = roster(m);
    for Outgoing {
        to,
        template,
        values,
    } in messages
    {
        let audience = audience_of(&roster, &to);
        if audience == 0 {
            continue;
        }
        let name = named(&roster);
        let line = arg_line(&template, &values, &name);
        for p in &roster {
            if let Some(user) = p.user
                && audience & show::player(p.slot) != 0
            {
                crate::hud_kit::notify(user, &line);
            }
        }
        if let Some(cue) = cue_of(ir, audience, &template, &values) {
            push(tick, cue);
        }
    }
}

/// The running script's declared type, message strings and tag table, from the variant record's blob:
/// compiled in or in the game-variant store (a `Viewer` has the blob from
/// the lobby fetch). Cached by the record's hash prefix.
fn script_context(m: &dyn GuestMemory) -> Option<(Type, Vec<String>, Vec<::rules::ir::TagDef>)> {
    let block = super::super::guest::block(m).ok().flatten()?;
    let script = variants::ext::Ext::in_effect(&block).script;
    // A kit director can run under an ordinary Slayer record. The
    // viewer already received its mode state with the actor packet.
    let kit_director = script.mode == variants::ext::Mode::None
        && super::super::guest::engine_index(m).ok().flatten() == Some(variants::GameType::Slayer.engine())
        && crate::firefight::hud::received().get().is_some_and(|blob| {
            crate::firefight::hud::Mode::decode(&blob).is_ok()
        });
    if script.mode == variants::ext::Mode::Firefight || kit_director {
        return Some((Type::Firefight, Vec::new(), Vec::new()));
    }
    if script.mode != variants::ext::Mode::Rules {
        return None;
    }
    type Cached = ([u8; 8], Type, Vec<String>, Vec<::rules::ir::TagDef>);
    static CACHE: Mutex<Option<Cached>> = Mutex::new(None);
    {
        let cache = lock(&CACHE);
        if let Some((prefix, ty, strings, tags)) = &*cache
            && *prefix == script.prefix
        {
            return Some((*ty, strings.clone(), tags.clone()));
        }
    }
    let blob = super::super::feature::local_blob(script.prefix)?;
    let (ir, _) = ::rules::verify::load(&blob).ok()?;
    let ty = ir.header.game_type;
    let strings = ir.strings;
    let tags = ir.tags;
    *lock(&CACHE) = Some((script.prefix, ty, strings.clone(), tags.clone()));
    Some((ty, strings, tags))
}

/// The cues a client received this tick: each decoded against the running
/// script's blob, checked, and shown to the local users in its audience.
/// A cue that fails any check is dropped whole and logged
/// (docs/game/rules-world-limits.md, "The show channel on a client").
pub async fn present(cx: &Cx, cues: &[ReceivedCue]) {
    for cue in cues {
        if let Err(why) = present_one(cx, cue).await {
            eprintln!(
                "rules: show cue at tick {} sequence {} dropped: {why}",
                cue.tick, cue.sequence
            );
        }
    }
}

/// The newest walls cue a client acted on, as (tick, sequence), and the
/// list it showed: a repeat of the same list, or an older cue arriving
/// late, changes nothing.
static WALLS_SEEN: Mutex<Option<((u32, u8), Vec<(u32, Option<u8>)>)>> = Mutex::new(None);

/// A walls cue on a client: the host's shown areas, drawn from this
/// machine's own copy of the map (`super::walls::client`).
fn present_walls(cx: &Cx, cue: &ReceivedCue) -> Result<(), String> {
    let Cue::Walls(shown) = Cue::decode(cue.kind, &cue.bytes, &[]).map_err(|e| e.to_string())? else {
        return Err("not a walls cue".into());
    };
    let key = (cue.tick, cue.sequence);
    {
        let mut seen = lock(&WALLS_SEEN);
        if let Some((at, last)) = &*seen
            && (key <= *at || *last == shown)
        {
            return Ok(());
        }
        *seen = Some((key, shown.clone()));
    }
    let walls = cx.read(|m| super::walls::client(m, &shown));
    crate::forge_glue::markers_world::publish_walls(walls);
    Ok(())
}

/// A pickers' offer cue on a client: the offer's strings read from the
/// running script's blob and held for the next tick boundary, when this
/// machine's local players are asked (`super::pick::client_tick`). An
/// offer with no rows takes the picker's offer away.
fn present_pick(cx: &Cx, cue: &ReceivedCue) -> Result<(), String> {
    let (_, strings, _) = cx
        .read(script_context)
        .ok_or_else(|| "the running script's blob is not on this machine".to_string())?;
    let Cue::Pick(offer) = Cue::decode(cue.kind, &cue.bytes, &strings).map_err(|e| e.to_string())? else {
        return Err("not an offer cue".into());
    };
    let text = |i: u16| {
        strings
            .get(usize::from(i))
            .cloned()
            .ok_or_else(|| format!("text {i} is not in the script's string table"))
    };
    let title = text(offer.title)?;
    let rows = offer
        .rows
        .iter()
        .map(|&r| text(r))
        .collect::<Result<Vec<_>, _>>()?;
    super::pick::client_offer(offer.slot, title, rows);
    Ok(())
}

async fn present_one(cx: &Cx, cue: &ReceivedCue) -> Result<(), String> {
    if cue.kind == show::WALLS {
        return present_walls(cx, cue);
    }
    if cue.kind == show::PICK {
        return present_pick(cx, cue);
    }
    let (ty, strings, tags) = cx
        .read(script_context)
        .ok_or_else(|| "the running script's blob is not on this machine".to_string())?;
    let message = match Cue::decode(cue.kind, &cue.bytes, &strings).map_err(|e| e.to_string())? {
        Cue::StartClip(clip) if clip == ty => {
            crate::medal_announcer::announce_clip(ty.name());
            return Ok(());
        }
        Cue::StartClip(_) => return Err("start clip is for another game type".into()),
        // A sound of the blob's tag table, played at this machine's local
        // players in its audience (`super::audio`).
        Cue::Sound(sound) => {
            sound.check(&tags).map_err(|e| e.to_string())?;
            let path = &tags[usize::from(sound.sound)].path;
            return super::audio::play(cx, sound.audience, path)
                .await
                .map_err(|e| e.to_string());
        }
        Cue::Message(message) => message,
        Cue::Walls(_) => return Err("a walls cue is read before the blob".into()),
        Cue::Pick(_) => return Err("an offer cue is read before the blob".into()),
    };
    let roster = cx.read(roster);
    let name = named(&roster);
    let template = &strings[usize::from(message.text)];
    let line = show::line(&strings, template, &message.values, &name);
    for p in &roster {
        if let Some(user) = p.user
            && message.audience & show::player(p.slot) != 0
        {
            crate::hud_kit::notify(user, &line);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::caps::id;

    fn ir() -> Ir {
        ::rules::compile(
            "rules \"M\" 1\ntype slayer\nrequires hud.message\n\
             global var n: int = 0\n\
             on tick { message(all, \"Wave {n}\") }\n",
        )
        .unwrap()
    }

    fn message_call(entry: u16, args: Vec<Arg>) -> ::rules::vm::Call {
        ::rules::vm::Call {
            entry: caps::entry(entry, caps::API_VERSION).unwrap(),
            args,
        }
    }

    #[test]
    fn an_outgoing_message_keeps_its_audience_template_and_values() {
        let call = message_call(
            id::MESSAGE,
            vec![
                Arg::Player(Some(3)),
                Arg::Text("Wave {int}".into()),
                Arg::Int(4),
            ],
        );
        assert_eq!(
            outgoing(&call),
            Some(Outgoing {
                to: To::Player(3),
                template: "Wave {int}".into(),
                values: vec![Arg::Int(4)],
            })
        );
        let team = message_call(
            id::MESSAGE_TEAM,
            vec![Arg::Team(Some(1)), Arg::Text("push".into())],
        );
        assert_eq!(outgoing(&team).map(|o| o.to), Some(To::Team(1)));
        let all = message_call(id::MESSAGE_ALL, vec![Arg::Text("go".into())]);
        assert_eq!(outgoing(&all).map(|o| o.to), Some(To::Everyone));
        // A call through `none` shows nothing.
        let none = message_call(id::MESSAGE, vec![Arg::Player(None), Arg::Text("hi".into())]);
        assert_eq!(outgoing(&none), None);
    }

    #[test]
    fn a_cue_names_only_what_the_blob_holds() {
        let ir = ir();
        let template = ir
            .strings
            .iter()
            .find(|s| s.contains("{int}"))
            .expect("the blob stores the message template")
            .clone();
        let cue = cue_of(&ir, show::EVERYONE, &template, &[Arg::Int(2)]).unwrap();
        let Cue::Message(message) = cue.clone() else { panic!("expected a message") };
        assert_eq!(message.audience, show::EVERYONE);
        assert_eq!(message.values, vec![HoleValue::Int(2)]);
        assert_eq!(ir.strings[usize::from(message.text)], template);
        // The wire form decodes to the same cue against the same table.
        let bytes = cue.encode().unwrap();
        assert_eq!(Cue::decode(cue.kind(), &bytes, &ir.strings).unwrap(), cue);
        // A template the blob does not hold makes no cue.
        assert!(cue_of(&ir, show::EVERYONE, "not in the table", &[]).is_none());
    }

    #[test]
    fn an_audience_resolves_against_the_roster() {
        let roster = vec![
            Roster {
                slot: 0,
                team: Some(0),
                name: "a".into(),
                user: Some(0),
            },
            Roster {
                slot: 2,
                team: Some(1),
                name: "b".into(),
                user: None,
            },
            Roster {
                slot: 5,
                team: Some(1),
                name: "c".into(),
                user: Some(2),
            },
        ];
        assert_eq!(audience_of(&roster, &To::Player(5)), show::player(5));
        assert_eq!(
            audience_of(&roster, &To::Team(1)),
            show::player(2) | show::player(5)
        );
        assert_eq!(audience_of(&roster, &To::Team(4)), 0);
        assert_eq!(audience_of(&roster, &To::Everyone), show::EVERYONE);
    }

    #[test]
    fn a_line_renders_player_names_from_the_roster() {
        let roster = vec![
            Roster {
                slot: 1,
                team: Some(0),
                name: "Ann".into(),
                user: Some(0),
            },
            Roster {
                slot: 2,
                team: Some(1),
                name: "Bo".into(),
                user: None,
            },
        ];
        let name = named(&roster);
        assert_eq!(
            arg_line(
                "{player} beat {player}",
                &[Arg::Player(Some(1)), Arg::Player(Some(2))],
                &name
            ),
            "Ann beat Bo"
        );
        // A slot that is not seated shows as `player N`.
        assert_eq!(
            arg_line("{player} left", &[Arg::Player(Some(9))], &name),
            "player 9 left"
        );
        assert_eq!(
            arg_line(
                "{team} in {time}",
                &[Arg::Team(Some(1)), Arg::Int(65 * caps::TICKS_PER_SECOND)],
                &name
            ),
            "team 1 in 1:05"
        );
    }

    /// A record is used only when the state it holds answers the blob: a
    /// widget or waypoint past the declarations, or a text outside the
    /// string table, is dropped whole (docs/game/rules-world-limits.md,
    /// "The show channel on a client").
    #[test]
    fn a_show_state_is_checked_against_the_blob_it_names() {
        let ir = ::rules::compile(
            "rules \"C\" 1\ntype slayer\nrequires hud.widgets, hud.waypoints\n\
             hud row: text at koth.hill\nwaypoint spot at ctf.mark\n\
             on round_start { hud_text(row, \"Ready\") }\n",
        )
        .unwrap();
        let mut state = State::default();
        *state.widget(0).unwrap() = ::rules::show::Widget {
            shown: true,
            text: Some(ir.strings.iter().position(|s| s == "Ready").unwrap() as u16),
            ..::rules::show::Widget::default()
        };
        state.waypoints[0] = Some(::rules::show::Waypoint {
            at: 7,
            audience: show::EVERYONE,
        });
        let bytes = state.encode();
        assert!(checked(&ir, &bytes).is_ok());

        // A widget past the one declared, a text outside the table, and a
        // waypoint past the one declared are each refused whole.
        let mut extra = state.clone();
        *extra.widget(5).unwrap() = ::rules::show::Widget::default();
        assert!(checked(&ir, &extra.encode()).is_err());
        let mut bad_text = state.clone();
        bad_text.widgets[0].as_mut().unwrap().text = Some(900);
        assert!(checked(&ir, &bad_text.encode()).is_err());
        let mut extra_wp = state.clone();
        extra_wp.waypoints[4] = Some(::rules::show::Waypoint {
            at: 9,
            audience: show::EVERYONE,
        });
        assert!(checked(&ir, &extra_wp.encode()).is_err());
    }

    #[test]
    fn the_channel_bounds_pending_cues() {
        clear_cues();
        for tick in 0..show::MAX_PENDING as u32 + 20 {
            push(
                tick,
                Cue::Message(Message {
                    audience: show::EVERYONE,
                    text: 0,
                    values: vec![],
                }),
            );
        }
        let cues = take_cues();
        assert_eq!(cues.len(), show::MAX_PENDING);
        // The oldest were dropped; the newest kept, with per-tick
        // sequences from 0.
        assert_eq!(cues[0].tick, 20);
        let last = cues.last().unwrap();
        assert_eq!(last.tick, show::MAX_PENDING as u32 + 19);
        assert_eq!(last.sequence, 0);
        assert!(take_cues().is_empty());
    }
}
