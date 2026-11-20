//! Xbox Live player options for a selected Clanker. See docs/game/bots.md.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use bots::{decisions::Personality, guest, mind::Skill, phantom};
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use live_proto::{Xuid, bots::is_bot_xuid};
use runtime::GuestFault;

use crate::list_class;
use crate::ui_kit::{
    dialog::DialogBase,
    dialog_tag, ids,
    input::{Glyph, Prompt},
    screen::{KitScreen, KitTexts, Row, Then},
    view::Opener,
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
}

impl Card {
    fn new(name: String, brain: Option<(Skill, Personality)>) -> Self {
        let Some((skill, p)) = brain else {
            return Self {
                name,
                difficulty: "Unavailable".into(),
                style: "Unavailable".into(),
            };
        };
        let traits = [
            (p.aggression, "Aggressive"),
            (p.caution, "Cautious"),
            (p.teamwork, "Team player"),
            (p.objective_focus, "Objective-minded"),
            (p.composure, "Composed"),
            (p.patience, "Patient"),
            (p.showboating, "Showboat"),
            (p.weapon_loyalty, "Weapon specialist"),
            (p.chattiness, "Chatty"),
        ];
        let style = traits
            .into_iter()
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap()
            .1;
        Self {
            name,
            difficulty: format!("{skill:?}"),
            style: style.into(),
        }
    }
}

fn route(selection: &Selection, known: Option<(String, Option<bots::BrainView>)>) -> Option<Card> {
    if let Some((name, brain)) = known {
        return Some(Card::new(name, brain.map(|v| (v.skill, v.personality))));
    }
    is_bot_xuid(selection.xuid()).then(|| Card::new(selection.name.clone(), None))
}

fn known(c: &Call, s: &Selection) -> Option<(String, Option<bots::BrainView>)> {
    if let Some(p) = phantom::list().into_iter().find(|p| {
        p.identity.xuid == s.xuid().0
            && (is_bot_xuid(s.xuid())
                || phantom::session::player_id(p.identity.xuid, p.machine) == s.id)
    }) {
        return Some((p.identity.gamertag, phantom::global().brain_view(p.handle)));
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
    marked_session_player(&*m, session, s).map(|name| (name, None))
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

fn field(current: &Current, get: fn(&Card) -> &String) -> String {
    let guard = current.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .map(|c| get(&c.lock().unwrap_or_else(|e| e.into_inner())).clone())
        .unwrap_or_default()
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
    let remote = known.as_ref().is_none_or(|(_, brain)| brain.is_none())
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
        let widget = widget.clone();
        live.bot_card(
            s.xuid(),
            Box::new(move |r| {
                let brain = r.skill.map(|k| {
                    let skill = crate::clankers::skill_of(k);
                    (skill, Personality::for_skill(r.seed, skill))
                });
                let mut old = card.lock().unwrap_or_else(|e| e.into_inner());
                let changed = old.name != r.gamertag.as_str();
                *old = Card::new(r.gamertag.to_string(), brain);
                if changed {
                    widget.refresh.store(true, Ordering::Release);
                }
            }),
        );
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
                use engine::datum::{DatumArray, layout};
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
        .zip([30i16, 120, -50, 200])
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
    let Some(block) = cx.read(|m| emblem_block(m, skin))? else {
        return Ok(());
    };
    let mut profile = [0; 16];
    profile[..8].copy_from_slice(&guest::bot_look(&name));
    cx.write(|m| {
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

pub fn install(
    registry: &mut hooks::Registry,
    live: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<()> {
    let current: Current = Arc::default();
    let widget = Arc::new(EmblemWidget::default());
    let (drawn, closing) = (widget.clone(), widget.clone());
    let (name, difficulty, style) = (current.clone(), current.clone(), current.clone());
    let page = Arc::new(
        KitScreen::new(
            "clanker-card",
            SCREEN,
            ids::family("clanker card"),
            "Clanker (bot)",
        )
        .dialog(DialogBase::Large, Opener::XBOX_LIVE_MENU)
        .row(Row::info("Gamertag", move || field(&name, |v| &v.name)))
        .row(Row::info("Difficulty", move || {
            field(&difficulty, |v| &v.difficulty)
        }))
        .row(Row::info("Style", move || field(&style, |v| &v.style)))
        .row(Row::action("Back", || Then::Close))
        .prompts(&[Prompt::new(&[Glyph::B], "Back")]),
    );
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
    use engine::testing::FakeGuest;

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
        assert_eq!(
            marked_session_player(&m, session, &s).as_deref(),
            Some("Bolt")
        );
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
        let card = Card::new("Rusty Lugnut".into(), Some((Skill::Heroic, p)));
        assert_eq!(card.name, "Rusty Lugnut");
        assert_eq!(card.difficulty, "Heroic");
        assert_eq!(card.style, "Team player");
        assert_eq!(Card::new(card.name, None).difficulty, "Unavailable");
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
