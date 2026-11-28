//! Xbox Live player options for a selected Clanker. See docs/game/bots.md.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use bots::{decisions::Personality, guest, mind::Skill, name_profile::NameProfile, phantom};
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use live_proto::{Xuid, bots::is_bot_xuid};
use runtime::GuestFault;

use crate::list_class;
use crate::ui_kit::{
    dialog::PICKER_ANIMATION,
    dialog_tag, ids,
    page::{ListPage, described},
    screen::{KitScreen, KitTexts, Row, Then},
};

pub const SCREEN: &str = "clanker_player_options";
const SELECTED: u32 = 0x0054_dc6c;

struct Selection {
    id: [u8; guest::ID_LEN],
    name: String,
}

impl Selection {
    fn read(m: &dyn GuestMemory) -> Option<Self> {
        // Kind 3 holds only a name, in the same bytes as the other kinds' XUID.
        if !matches!(m.u32(SELECTED).ok()?, 1 | 2) {
            return None;
        }
        let mut id = [0; guest::ID_LEN];
        m.read(SELECTED + 4, &mut id).ok()?;
        let mut label = [0; 16];
        m.read(SELECTED + 16, &mut label).ok()?;
        let end = label.iter().position(|b| *b == 0).unwrap_or(label.len());
        Some(Self {
            id,
            name: String::from_utf8_lossy(&label[..end]).into_owned(),
        })
    }

    fn xuid(&self) -> Xuid {
        Xuid(u64::from_le_bytes(self.id[..8].try_into().unwrap()))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Card {
    name: String,
    difficulty: String,
    style: String,
    has_difficulty: bool,
    has_style: bool,
    profile: String,
    loading: bool,
}

impl Card {
    /// On the host, display the running brain's skill and temperament.
    /// A rostered card knows its last skill and derives the same temperament
    /// from the full account name. The named profile also works offline.
    fn new(name: String, skill: Option<Skill>, traits: Option<Personality>) -> Self {
        let profile = NameProfile::for_name(&name);
        let style = traits.or_else(|| skill.map(|k| profile.personality(k)));
        Self {
            difficulty: skill.map_or_else(|| "Unavailable".into(), |k| format!("{k:?}")),
            style: style.map_or("Unavailable", strongest_trait).into(),
            has_difficulty: skill.is_some(),
            has_style: style.is_some(),
            profile: if name.trim().is_empty() {
                "Profile unavailable".into()
            } else {
                profile.summary()
            },
            name,
            loading: false,
        }
    }

    fn account(account: live_proto::bots::PlayerCard) -> Self {
        Self::new(
            account.gamertag.to_string(),
            account.skill.map(crate::clankers::skill_of),
            None,
        )
    }

    fn skill_detail<'a>(&self, value: &'a str) -> &'a str {
        if self.loading {
            "Loading..."
        } else {
            value
        }
    }

    fn finish_lookup(&mut self, card: Option<live_proto::bots::PlayerCard>) -> bool {
        self.loading = false;
        if let Some(card) = card {
            let resolved = Self::account(card);
            let changed = self.name != resolved.name;
            *self = resolved;
            changed
        } else {
            false
        }
    }
}

fn strongest_trait(p: Personality) -> &'static str {
    [
        (p.aggression, "Aggressive"),
        (p.caution, "Cautious"),
        (p.teamwork, "Team player"),
        (p.objective_focus, "Objective-minded"),
        (p.composure, "Composed"),
        (p.patience, "Patient"),
        (p.showboating, "Showboat"),
        (p.weapon_loyalty, "Weapon specialist"),
        (p.chattiness, "Chatty"),
    ]
    .into_iter()
    .max_by(|a, b| a.0.total_cmp(&b.0))
    .unwrap()
    .1
}

enum Known {
    Hosted(String, Option<bots::BrainView>),
    Remote(String),
}

fn route(selection: &Selection, known: Option<Known>) -> Option<Card> {
    match known {
        Some(Known::Hosted(name, Some(brain))) => {
            Some(Card::new(name, Some(brain.skill), Some(brain.personality)))
        }
        Some(Known::Hosted(name, None)) => {
            let mut card = Card::new(name, None, None);
            card.profile = "Unavailable".into();
            Some(card)
        }
        Some(Known::Remote(name)) => Some(Card::new(name, None, None)),
        None => is_bot_xuid(selection.xuid()).then(|| Card::new(selection.name.clone(), None, None)),
    }
}

fn hosted(phantoms: &phantom::Phantoms, s: &Selection) -> Option<Known> {
    let p = phantoms.list().into_iter().find(|p| {
        p.identity.xuid == s.xuid().0
            && (is_bot_xuid(s.xuid())
                || phantom::session::player_id(p.identity.xuid, p.machine) == s.id)
    })?;
    Some(Known::Hosted(p.identity.gamertag, phantoms.brain_view(p.handle)))
}

fn known(c: &Call, s: &Selection) -> Option<Known> {
    if let Some(p) = hosted(phantom::global(), s) {
        return Some(p);
    }
    let m = c.ctx.runtime().memory();
    if m.u8(points::NETWORK_READY.base).ok()? == 0 {
        return None;
    }
    let session = bots::lobby::game_session(&*m).ok().filter(|s| *s != 0)?;
    let state = m.u32(session + phantom::session::at::STATE).ok()?;
    if !(3..=8).contains(&state) {
        return None;
    }
    marked_session_player(&*m, session, s).map(Known::Remote)
}

/// A guest bot on another console may share an account XUID with its human.
/// The effective, gear-marked name belongs to this exact session player id.
fn marked_session_player(
    m: &dyn GuestMemory,
    session: u32,
    selection: &Selection,
) -> Option<String> {
    use phantom::session::at::*;
    if selection.id == [0; guest::ID_LEN] {
        return None;
    }
    let mask = m.u32(session + PLAYER_MASK).ok()?;
    for i in (0..16).filter(|i| mask & (1 << i) != 0) {
        let p = session + PLAYERS + i * PLAYER_SIZE;
        let mut id = [0; guest::ID_LEN];
        m.read(p + P_ID, &mut id).ok()?;
        if id != selection.id {
            continue;
        }
        let mut units = Vec::new();
        for j in 0..CONFIG_NAME_UNITS {
            let u = m.u16(p + P_EFFECTIVE + 2 * j as u32).ok()?;
            if u == 0 {
                break;
            }
            units.push(u);
        }
        if units.first() == Some(&(live_proto::bots::BOT_MARK as u16)) {
            return Some(String::from_utf16_lossy(live_proto::bots::unmarked_units(
                &units,
            )));
        }
    }
    None
}

// The recent-players list copies 0x58-byte history records into row datums
// at +4. Its text widget consumes the wide name. Older records may contain
// the ASCII replacement for the gear. XUID identifies the bot in that case.
fn recent_name(m: &dyn GuestMemory, row: u32) -> Option<[u8; 64]> {
    let mut id = [0; 8];
    m.read(row + 4, &mut id).ok()?;
    if !is_bot_xuid(Xuid(u64::from_le_bytes(id))) {
        return None;
    }
    let at = row + 0x10;
    // Most rows already carry the canonical prefix.
    if m.u16(at).ok()? == phantom::session::MARK as u16
        && m.u16(at + 2).ok()? == 0x20
        && m.u16(at + 4).ok()? != 0x20
    {
        return None;
    }
    let name = m.utf16(at, 31).ok()?;
    let tag = name
        .strip_prefix([phantom::session::MARK, '?'])
        .unwrap_or(&name)
        .trim_start();
    if tag.is_empty() {
        return None;
    }
    let canonical = phantom::session::marked_name(tag);
    let mut bytes = [0; 64];
    for (dst, unit) in bytes.chunks_exact_mut(2).zip(canonical) {
        dst.copy_from_slice(&unit.to_le_bytes());
    }
    Some(bytes)
}

// Both Live selection paths pack a wide label into a 16-byte account field.
// Strip the display decoration before retail narrows and truncates it.
fn account_name_source(m: &dyn GuestMemory, caller: u32, src: u32) -> u32 {
    if matches!(caller, 0x002b_3e46 | 0x0023_411e)
        && m.u16(src).ok() == Some(phantom::session::MARK as u16)
    {
        let space = m.u16(src + 2).ok() == Some(0x20);
        return src + if space { 4 } else { 2 };
    }
    src
}

// The card is a modal on the shared dialog window. A lookup holds only its
// own card, so a late server reply cannot change the next selection.
type Current = Arc<Mutex<Option<Arc<Mutex<Card>>>>>;

fn card_value<T>(current: &Current, get: impl FnOnce(&Card) -> T) -> Option<T> {
    let current = current.lock().unwrap_or_else(|e| e.into_inner());
    let card = current.as_ref()?.lock().unwrap_or_else(|e| e.into_inner());
    Some(get(&card))
}

fn field(current: &Current, get: fn(&Card) -> &str) -> String {
    card_value(current, |card| get(card).to_owned()).unwrap_or_default()
}

#[derive(Default)]
struct EmblemWidget {
    addr: AtomicU32,
    refresh: AtomicBool,
}

struct Options {
    widget: Arc<EmblemWidget>,
    current: Current,
    live: Option<Arc<live_hle::Hle>>,
}

fn show(
    c: &mut Call,
    current: &Current,
    live: Option<&live_hle::Hle>,
    widget: &Arc<EmblemWidget>,
) -> Result<Flow, GuestFault> {
    let selection = { Selection::read(&*c.ctx.runtime().memory()) };
    let Some(s) = selection else {
        return Ok(Flow::Continue);
    };
    let known = known(c, &s);
    let remote = !matches!(&known, Some(Known::Hosted(..)))
        && live_proto::bots::is_server_xuid(s.xuid());
    let Some(card) = route(&s, known) else {
        return Ok(Flow::Continue);
    };
    let dialog = list_class::loader_of(SCREEN).and_then(dialog_tag::by_loader);
    let mask = c.ctx.runtime().memory().u16(c.cpu.eax + 2).unwrap_or(0);
    // Once identified, a missing class or failed allocation cannot expose retail actions.
    c.cpu.eax = 0;
    let Some(d) = dialog.filter(|_| mask != 0) else {
        return c.ret(0).map(Flow::Return);
    };
    let card = Arc::new(Mutex::new(card));
    {
        let mut open = current.lock().unwrap_or_else(|e| e.into_inner());
        if open.is_some() {
            drop(open);
            return c.ret(0).map(Flow::Return);
        }
        *open = Some(card.clone());
    }
    if remote && let Some(live) = live {
        card.lock().unwrap_or_else(|e| e.into_inner()).loading = true;
        let loading = card.clone();
        let widget = widget.clone();
        if live
            .bot_card(
                s.xuid(),
                Box::new(move |r| {
                    let mut card = loading.lock().unwrap_or_else(|e| e.into_inner());
                    if card.finish_lookup(r.ok()) {
                        widget.refresh.store(true, Ordering::Release);
                    }
                }),
            )
            .is_err()
        {
            card.lock()
                .unwrap_or_else(|e| e.into_inner())
                .finish_lookup(None);
        }
    }
    let current = current.clone();
    c.answer_async(move |cx| {
        Box::pin(async move {
            let at = d.page + dialog_tag::REQUEST_AT;
            let result = async {
                cx.write(|m| {
                    m.write(
                        at,
                        &dialog_tag::request(dialog_tag::DIALOG_WINDOW, mask, d.loader),
                    )
                })?;
                cx.call(GuestCall::new(d.loader).args([at])).await
            }
            .await;
            if !matches!(&result, Ok(regs) if regs.eax != 0) {
                current.lock().unwrap_or_else(|e| e.into_inner()).take();
            }
            Ok(Ret::new(result?.eax, 0))
        })
    })
}

impl hooks::Feature for Options {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "clanker-options-route",
            "Xbox Live player options for Clankers",
        )
        .doc("docs/game/bots.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.raw().after(
            &points::RECENT_PLAYERS_BUILD,
            "restore bot display names in the recent history rows",
            |c| {
                use guest_layout::datum::{DatumArray, layout};
                let m = &mut *c.ctx.memory();
                let array = m.u32(c.cpu.eax + 0x70)?;
                if array != 0 {
                    let rows = DatumArray::at(array);
                    let count = m.u32(array + layout::HIGH_WATER)? as i32;
                    for i in 0..count.min(100) {
                        if let Ok(Some(row)) = rows.get_absolute(m, i)
                            && let Some(name) = recent_name(m, row)
                        {
                            m.write(row + 0x10, name)?;
                        }
                    }
                }
                Ok(())
            },
        );
        b.raw()
            .before(
                &points::PLAYER_NAME_TO_ASCII,
                "pack the account gamertag",
                |c| {
                    let m = c.ctx.memory();
                    let caller = m.u32(c.cpu.esp)?;
                    c.cpu.eax = account_name_source(&*m, caller, c.cpu.eax);
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        for point in [
            &points::PLAYER_OPTIONS_CHECK_FRIEND,
            &points::PLAYER_OPTIONS_OPEN,
        ] {
            let (current, live, widget) =
                (self.current.clone(), self.live.clone(), self.widget.clone());
            b.raw()
                .before(point, "show the selected Clanker", move |c| {
                    show(c, &current, live.as_deref(), &widget)
                });
        }
        let (widget, current) = (self.widget.clone(), self.current.clone());
        b.raw().before(
            &points::EMBLEM_WIDGET_DRAW,
            "refresh a resolved Clanker gamertag",
            move |c| {
                if widget.addr.load(Ordering::Relaxed) == c.cpu.ecx
                    && c.cpu.ecx != 0
                    && widget.refresh.swap(false, Ordering::AcqRel)
                {
                    let name = field(&current, |v| &v.name);
                    c.ctx
                        .runtime()
                        .memory()
                        .write(c.cpu.ecx + 0x74, guest::bot_look(&name))?;
                }
                Ok(Flow::Continue)
            },
        );
        let current = self.current.clone();
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            current
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
                .then(|| "Clanker player card open".into())
        }));
        Ok(())
    }
}

pub(crate) const EMBLEM_RECT: [i16; 4] = [30, 120, -50, 200];

// Skin 6 is the Live players list in both interfaces. Its first table's
// skin (player_skin_ymenu) contains the retail medium emblem shader.
fn emblem_block(m: &dyn GuestMemory, skin: u32) -> Result<Option<[u8; 0x24]>, MemFault> {
    if skin == 0 || m.u32(skin + 0x34)? == 0 {
        return Ok(None);
    }
    let table = m.u32(skin + 0x38)?;
    let Some(row) = dialog_tag::tag_of(m, m.u32(table + 8)?)? else {
        return Ok(None);
    };
    if m.u32(row + 0x2c)? == 0 {
        return Ok(None);
    }
    let mut block = [0; 0x24];
    m.read(m.u32(row + 0x30)?, &mut block)?;
    // This is a player emblem even when the lobby uses team colours.
    block[0] &= !2;
    for (pair, coord) in block[0x1c..]
        .chunks_exact_mut(2)
        .zip(EMBLEM_RECT)
    {
        pair.copy_from_slice(&coord.to_le_bytes());
    }
    Ok(Some(block))
}

async fn add_emblem(
    cx: Cx,
    screen: u32,
    name: String,
    drawn: Arc<EmblemWidget>,
) -> Result<(), EntryError> {
    let table = cx.read(|m| m.u32(screen))?;
    let Some(d) = dialog_tag::by_vtable(table) else {
        return Ok(());
    };
    const BLOCK: u32 = 0xf00;
    const PROFILE: u32 = BLOCK + 0x24;
    const _: () = assert!(
        dialog_tag::base_tag_size(dialog_tag::BASE_BITMAPS, 0) <= BLOCK
            && PROFILE + 16 <= dialog_tag::REQUEST_AT
    );
    let skin = cx
        .call(GuestCall::new(points::UI_LIST_SKIN.base).args([6]))
        .await?
        .eax;
    let Some(mut block) = cx.read(|m| emblem_block(m, skin))? else {
        return Ok(());
    };
    let mut profile = [0; 16];
    profile[..8].copy_from_slice(&guest::bot_look(&name));
    cx.write(|m| {
        let animation = list_class::widget_animation_index(m, screen)?;
        block[4..6].copy_from_slice(&animation.to_le_bytes());
        m.write(d.page + BLOCK, &block)?;
        m.write(d.page + PROFILE, &profile)
    })?;
    let widget = cx
        .call(
            GuestCall::new(points::PANE_SHADER_ADD.base)
                .esi(screen)
                .ebx(d.page + BLOCK),
        )
        .await?
        .eax;
    drawn.addr.store(widget, Ordering::Relaxed);
    if widget != 0 {
        cx.call(
            GuestCall::new(points::EMBLEM_WIDGET_SET.base)
                .eax(widget)
                .edx(u32::MAX)
                .ecx(u32::MAX)
                .args([d.page + PROFILE]),
        )
        .await?;
    }
    Ok(())
}

fn card_screen(current: &Current) -> KitScreen {
    let (name, difficulty, style, summary) =
        (current.clone(), current.clone(), current.clone(), current.clone());
    // The profile line goes in the doc's description block: inside the
    // panel below the title, above the emblem and clear of the list.
    KitScreen::new("clanker-card", SCREEN, ids::family("clanker card"))
    .row(Row::info("Gamertag", move || field(&name, |v| &v.name)))
    .row(Row::info_from_source("Difficulty", move || {
        card_value(&difficulty, |v| {
            (v.skill_detail(&v.difficulty).to_owned(), v.has_difficulty && !v.loading)
        }).unwrap_or_default()
    }))
    .row(Row::info_from_source("Style", move || {
        card_value(&style, |v| {
            (v.skill_detail(&v.style).to_owned(), v.has_style && !v.loading)
        }).unwrap_or_default()
    }))
    .row(Row::action("Back", || Then::Close))
    .live_help(move || field(&summary, |v| &v.profile))
    .description_animation(PICKER_ANIMATION)
}

pub(crate) fn page_sample() -> ListPage {
    let card = Card::new("Rusty Bolt".into(), Some(Skill::Heroic), None);
    let current = Arc::new(Mutex::new(Some(Arc::new(Mutex::new(card)))));
    let screen = Arc::new(card_screen(&current));
    ListPage::new(screen.class(), described(&screen))
}

pub fn install(
    registry: &mut hooks::Registry,
    live: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<()> {
    let current: Current = Arc::default();
    let widget = Arc::new(EmblemWidget::default());
    let (drawn, closing) = (widget.clone(), widget.clone());
    let page = Arc::new(card_screen(&current));
    let (freed, names, open) = (page.on_free(), current.clone(), current.clone());
    let class = page
        .class()
        .pane_ready(Arc::new(move |cx, screen| {
            Box::pin(add_emblem(
                cx,
                screen,
                field(&names, |v| &v.name),
                drawn.clone(),
            ))
        }))
        .on_free(Arc::new(move |cx, screen| {
            closing.addr.store(0, Ordering::Relaxed);
            open.lock().unwrap_or_else(|e| e.into_inner()).take();
            freed(cx, screen)
        }));
    registry
        .add(&KitTexts(page))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    registry.add(&class).map_err(|e| anyhow::anyhow!("{e}"))?;
    registry
        .add(&Options {
            current,
            live,
            widget,
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn options_route_uses_the_selected_identity_and_never_the_spelling() {
        let mut mem = FakeGuest::new();
        mem.map(SELECTED, 120).put(SELECTED + 16, b"Rusty Lugnut\0");
        for (kind, uid, card) in [
            (1, live_proto::bots::local_xuid(3).0, true),
            (2, live_proto::bots::SERVER_XUID_MARK | 91, true),
            (1, 0x0009_0000_0000_0003, false),
            (3, live_proto::bots::SERVER_XUID_MARK | 91, false),
        ] {
            mem.put_u32(SELECTED, kind)
                .put(SELECTED + 4, &uid.to_le_bytes());
            let model = Selection::read(&mem).and_then(|s| route(&s, None));
            assert_eq!(model.is_some(), card);
            if let Some(m) = model {
                assert_eq!(m.name, "Rusty Lugnut");
                assert_eq!(m.profile, NameProfile::for_name(&m.name).summary());
            }
        }
    }

    #[test]
    fn a_remote_guest_sharing_a_human_xuid_needs_its_own_marked_session_slot() {
        use phantom::session::at::*;
        let session = 0x10000;
        let mut m = FakeGuest::new();
        m.map(session, 0x4000).put_u32(session + PLAYER_MASK, 1);
        let mut s = Selection {
            id: [0; 12],
            name: "Host".into(),
        };
        s.id[0] = 23;
        s.id[8] = 2;
        let p = session + PLAYERS;
        m.put(p, &s.id);
        assert!(marked_session_player(&m, session, &s).is_none());
        for (i, u) in phantom::session::marked_name("Bolt")
            .into_iter()
            .enumerate()
        {
            m.put_u16(p + P_EFFECTIVE + 2 * i as u32, u);
        }
        let name = marked_session_player(&m, session, &s).unwrap();
        let card = route(&s, Some(Known::Remote(name))).unwrap();
        let scripted = route(&s, Some(Known::Hosted("Bolt".into(), None))).unwrap();
        assert_eq!(scripted.profile, "Unavailable");
        assert_eq!(card.name, "Bolt");
        assert_eq!(card.profile, NameProfile::for_name("Bolt").summary());
        s.id[8] = 0;
        assert!(marked_session_player(&m, session, &s).is_none());
    }

    #[test]
    fn recent_bot_names_use_the_wide_gear_and_pack_the_whole_account_tag() {
        let row = 0x4000;
        let mut m = FakeGuest::new();
        m.map(row, 0x5c)
            .put(row + 4, &live_proto::bots::local_xuid(3).0.to_le_bytes());
        let at = row + 0x10;
        for prefix in ["", "?", "? ", "⚙", "⚙  "] {
            m.put(
                at,
                &guest::profile_name_bytes(&format!("{prefix}Fifteen Chars!!")),
            );
            let bytes = recent_name(&m, row).unwrap();
            m.put(at, &bytes);
            assert_eq!(m.utf16(at, 31).unwrap(), "⚙ Fifteen Chars!!");
            assert!(recent_name(&m, row).is_none());
            for caller in [0x002b_3e46, 0x0023_411e] {
                let src = account_name_source(&m, caller, at);
                assert_eq!(m.utf16(src, 15).unwrap(), "Fifteen Chars!!");
            }
            assert_eq!(account_name_source(&m, 0, at), at);
        }
        m.put(row + 4, &1u64.to_le_bytes())
            .put(at, &guest::profile_name_bytes("? Human"));
        assert!(recent_name(&m, row).is_none());
        assert_eq!(account_name_source(&m, 0x002b_3e46, at), at);
    }

    #[test]
    fn card_shows_the_active_difficulty_and_strongest_trait() {
        let mut p = Personality::for_skill(73, Skill::Heroic);
        p.teamwork = 1.0;
        let card = Card::new("Rusty Lugnut".into(), Some(Skill::Heroic), Some(p));
        assert_eq!(card.name, "Rusty Lugnut");
        assert_eq!(card.difficulty, "Heroic");
        assert_eq!(card.style, "Team player");
        let pending = Card::new(card.name, None, None);
        assert_eq!(pending.difficulty, "Unavailable");
        assert_eq!(pending.profile, card.profile);
    }

    #[test]
    fn a_hosted_clanker_uses_its_active_brain_and_full_name() {
        let phantoms = phantom::Phantoms::default();
        let xuid = live_proto::bots::local_xuid(1);
        let name = "Copper Circuit";
        let brain = bots::Brain::new(Skill::Legendary, 1).with_name_profile(name);
        let style = strongest_trait(brain.view().personality);
        phantoms
            .add(
                phantom::Identity {
                    xuid: xuid.0,
                    gamertag: name.into(),
                },
                None,
                Box::new(brain),
            )
            .unwrap();
        let mut selection = Selection {
            id: [0; guest::ID_LEN],
            name: "Copper".into(),
        };
        selection.id[..8].copy_from_slice(&xuid.0.to_le_bytes());
        let card = route(&selection, hosted(&phantoms, &selection)).unwrap();
        assert_eq!(card.name, name);
        assert_eq!(card.difficulty, "Legendary");
        assert_eq!(card.style, style);
        assert_eq!(card.profile, NameProfile::for_name(name).summary());
    }

    #[test]
    fn a_lookup_shows_loading_until_it_succeeds_or_fails() {
        use crate::ui_kit::spec::Describe;

        let card = Arc::new(Mutex::new(Card::new("Copper".into(), None, None)));
        let current = Arc::new(Mutex::new(Some(card.clone())));
        let screen = card_screen(&current);
        let values = || {
            let spec = screen.spec();
            [
                spec.rows[1].plain_value().unwrap(),
                spec.rows[2].plain_value().unwrap(),
            ]
        };
        card.lock().unwrap().loading = true;
        assert_eq!(values(), ["Loading...", "Loading..."]);
        assert!(!card.lock().unwrap().finish_lookup(None));
        assert_eq!(values(), ["Unavailable", "Unavailable"]);

        card.lock().unwrap().loading = true;
        let account = live_proto::bots::PlayerCard {
            xuid: Xuid(live_proto::bots::SERVER_XUID_MARK | 81),
            gamertag: live_proto::Gamertag::new("Copper Circuit").unwrap(),
            seed: 9,
            skill: Some(live_proto::bots::HEROIC),
        };
        let profile = NameProfile::for_name("Copper Circuit");
        let style = strongest_trait(profile.personality(Skill::Heroic));
        assert!(card.lock().unwrap().finish_lookup(Some(account)));
        assert_eq!(values(), ["Heroic", style]);
        assert_eq!(
            screen.spec().rows[0].plain_value().as_deref(),
            Some("Copper Circuit")
        );
    }

    #[test]
    fn a_rostered_card_uses_the_full_name_and_last_skill_like_the_host() {
        let name = "Silver Circuit";
        let profile = NameProfile::for_name(name);
        let mut account = live_proto::bots::PlayerCard {
            xuid: Xuid(live_proto::bots::SERVER_XUID_MARK | 81),
            gamertag: live_proto::Gamertag::new(name).unwrap(),
            seed: 9,
            skill: None,
        };
        let waiting = Card::account(account.clone());
        assert_eq!(waiting.profile, profile.summary());
        assert_eq!(waiting.difficulty, "Unavailable");
        let selection = Selection { id: [0; guest::ID_LEN], name: name.into() };
        for (skill, value) in Skill::ALL.into_iter().zip([
            live_proto::bots::EASY,
            live_proto::bots::NORMAL,
            live_proto::bots::HEROIC,
            live_proto::bots::LEGENDARY,
        ]) {
            let brain = bots::Brain::new(skill, 73).with_name_profile(name);
            let host = route(&selection, Some(Known::Hosted(name.into(), Some(brain.view())))).unwrap();
            account.skill = Some(value);
            for seed in [0, u64::MAX] {
                account.seed = seed;
                assert_eq!(Card::account(account.clone()), host);
            }
        }
    }

    #[test]
    fn the_profile_line_refreshes_without_moving_focus_and_preserves_the_card_rows() {
        use crate::list_class::Rows;
        use crate::ui_kit::{
            dialog::{DialogBase, DialogGeometry},
            spec::Describe,
            view::{Metrics, Synthetic},
        };

        let card = Arc::new(Mutex::new(Card::new("Rusty Bolt".into(), None, None)));
        let current = Arc::new(Mutex::new(Some(Arc::clone(&card))));
        let screen = card_screen(&current);
        let spec = screen.spec();
        let labels: Vec<_> = spec.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Gamertag", "Difficulty", "Style", "Back"]);
        let initial = spec.description_text().unwrap();
        assert_eq!(initial, NameProfile::for_name("Rusty Bolt").summary());
        assert!(screen.live_text().redraw);
        assert!(!screen.live_text().redraw);

        let resolved = (0..128)
            .map(|i| Card::new(format!("Circuit {i}"), Some(Skill::Heroic), None))
            .find(|c| c.profile != initial)
            .unwrap();
        let updated = resolved.profile.clone();
        *card.lock().unwrap() = resolved;
        assert!(screen.live_text().redraw);
        assert!(!screen.live_text().redraw);
        assert_eq!(screen.spec().description_text().as_deref(), Some(updated.as_str()));

        let profile = screen.spec().description.expect("the profile line's block");
        let [top, left, bottom, right] = profile.rect;
        let geometry = DialogGeometry::fallback_for(DialogBase::Large);
        let list_right = geometry.list.anchor[0] + geometry.rows.label.0.right;
        assert!(left > list_right);
        assert!(bottom > EMBLEM_RECT[0]);
        assert!(top < geometry.header.bottom && right <= geometry.header.right);
        assert!(Synthetic.line_height(profile.font) <= i32::from(top - bottom));
        for i in 0..256 {
            let text = NameProfile::for_name(&format!("Circuit {i}")).summary();
            assert!(!text.contains('\n'));
            assert!(Synthetic.width(profile.font, &text) <= i32::from(right - left));
        }
    }

    #[test]
    fn card_reuses_the_players_list_emblem_material_and_owns_its_rectangle() {
        let (skin, tables, entries, row, block) = (0x1000, 0x2000, 0x3000, 0x4000, 0x5000);
        let mut m = FakeGuest::new();
        m.map(skin, 0x5000)
            .map(points::TAG_ENTRIES.base, 4)
            .put_u32(points::TAG_ENTRIES.base, entries)
            .put_u32(skin + 0x34, 1)
            .put_u32(skin + 0x38, tables)
            .put_u32(tables + 8, 0xc0010002)
            .put_u32(entries + 0x28, row)
            .put_u32(row + 0x2c, 1)
            .put_u32(row + 0x30, block)
            .put_u32(block, 3)
            .put_u32(block + 24, 0xfeed1234);
        let copy = emblem_block(&m, skin).unwrap().unwrap();
        assert_eq!(copy[0], 1);
        assert_eq!(&copy[24..28], &0xfeed1234u32.to_le_bytes());
        assert_eq!(&copy[28..30], &30i16.to_le_bytes());
        assert_eq!(m.u32(block).unwrap(), 3);
        m.put_u32(row + 0x2c, 0);
        assert!(emblem_block(&m, skin).unwrap().is_none());
    }
}
