//! The page a specimen or a layout steps through: the focus moves with up
//! and down, left and right step a value (or move a code's cursor), up and
//! down change a code's digit, A presses a row that has a press, B and
//! Back close. A press says what it did; nothing outside the page
//! changes.

use crate::ui_kit::input::Input;
use crate::ui_kit::page::{Driven, Live, Page, focus_line};
use crate::ui_kit::spec::{Press, RowKind, RowSpec, ScreenSpec};

/// What a step did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Focus(usize),
    /// A value stepped: `label = value`.
    Stepped(String),
    /// A row (or a button) pressed.
    Pressed(String),
    /// A code's cursor moved.
    Cursor(usize),
    /// A code's digit changed: the cell and its digit.
    Digit(usize, String),
    Close,
}

/// A page of named states, each a spec.
pub struct Specimen {
    states: Vec<(String, ScreenSpec)>,
}

impl Specimen {
    pub fn new(states: Vec<(String, ScreenSpec)>) -> Specimen {
        Specimen { states }
    }
    pub fn live(self) -> Box<dyn Live> {
        Box::new(Driven::new(self))
    }
}

fn rows(s: &ScreenSpec) -> &[RowSpec] {
    s.dialog.as_ref().map_or(&s.rows, |d| &d.rows)
}

/// Steps the focused value row of `s` (its rows and its dialog's).
fn step_value(s: &mut ScreenSpec, right: bool) -> Effect {
    let focus = s.focus;
    let mut line = None;
    let lists: Vec<&mut Vec<RowSpec>> = match &mut s.dialog {
        Some(d) => vec![&mut s.rows, &mut d.rows],
        None => vec![&mut s.rows],
    };
    for rows in lists {
        if let Some(r) = rows.get_mut(focus)
            && let RowKind::Value { values, current } = &mut r.kind
            && values.len() > 1
        {
            let n = values.len();
            *current = if right { (*current + 1) % n } else { (*current + n - 1) % n };
            line = Some(format!("{} = {}", r.label, values[*current]));
        }
    }
    line.map_or(Effect::None, Effect::Stepped)
}

impl Page for Specimen {
    type State = ScreenSpec;
    type Event = Input;
    type Effect = Effect;

    fn step(&self, s: &mut ScreenSpec, input: Input) -> Effect {
        if !s.inputs.contains(&input) {
            return Effect::None;
        }
        if let Some(cells) = s.dialog.as_mut().and_then(|d| d.cells.as_mut())
            && let Some(cursor) = cells.cursor
        {
            match input {
                Input::Left if cursor > 0 => {
                    cells.cursor = Some(cursor - 1);
                    return Effect::Cursor(cursor - 1);
                }
                Input::Right if cursor + 1 < cells.text.len() => {
                    cells.cursor = Some(cursor + 1);
                    return Effect::Cursor(cursor + 1);
                }
                Input::Up | Input::Down => {
                    let d = cells.text[cursor].parse::<u8>().unwrap_or(0);
                    let d = if input == Input::Up { (d + 1) % 10 } else { (d + 9) % 10 };
                    cells.text[cursor] = d.to_string();
                    return Effect::Digit(cursor, cells.text[cursor].clone());
                }
                _ => {}
            }
        }
        let n = rows(s).len();
        match input {
            Input::Up if s.focus > 0 => {
                s.focus -= 1;
                Effect::Focus(s.focus)
            }
            Input::Down if s.focus + 1 < n => {
                s.focus += 1;
                Effect::Focus(s.focus)
            }
            Input::Left | Input::Right => step_value(s, input == Input::Right),
            Input::A => match rows(s).get(s.focus) {
                Some(r) if r.press != Press::Nothing => Effect::Pressed(r.label.clone()),
                Some(_) => Effect::None,
                None => Effect::Pressed("A".into()),
            },
            Input::B | Input::Back => Effect::Close,
            Input::X | Input::Y | Input::Start => Effect::Pressed(format!("{input:?}")),
            _ => Effect::None,
        }
    }

    fn handled(effect: &Effect) -> bool {
        *effect != Effect::None
    }

    fn spec(&self, s: &ScreenSpec) -> ScreenSpec {
        s.clone()
    }

    fn states(&self) -> Vec<(String, ScreenSpec)> {
        self.states.clone()
    }

    fn summary(&self, s: &ScreenSpec) -> String {
        format!("{} | {}", s.title, focus_line(s))
    }
}
