//! Scenario help is a Unicode list with percent-prefixed input tokens.

use crate::h1::{HudMessage, MessagePart, Ustr};

pub(crate) fn messages(list: Ustr) -> Vec<HudMessage> {
    list.strings
        .iter()
        .enumerate()
        .map(|(index, text)| HudMessage {
            name: format!("scenario_help_{index}"),
            parts: parts(text),
        })
        .collect()
}

fn parts(text: &str) -> Vec<MessagePart> {
    let mut output = Vec::new();
    let mut run = String::new();
    let mut units = 0;
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        rest = &rest[ch.len_utf8()..];
        if ch == '%' {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphabetic() || c == '-'))
                .unwrap_or(rest.len());
            let glyph = match &rest[..end] {
                "action" => Some(18),
                "throw-grenade" => Some(19),
                "primary-trigger" => Some(20),
                "use-equipment" => Some(21),
                "rotate-weapons" => Some(24),
                "crouch" => Some(27),
                "move" => Some(30),
                "look" => Some(31),
                "left-stick" => Some(16),
                "right-stick" => Some(17),
                _ => None,
            };
            if let Some(glyph) = glyph {
                if !run.is_empty() {
                    output.push(MessagePart::Text(std::mem::take(&mut run)));
                    units = 0;
                }
                output.push(MessagePart::Glyph(glyph));
                rest = &rest[end..];
                continue;
            }
        }
        if units + ch.len_utf16() > 254 {
            output.push(MessagePart::Text(std::mem::take(&mut run)));
            units = 0;
        }
        run.push(ch);
        units += ch.len_utf16();
    }
    if !run.is_empty() {
        output.push(MessagePart::Text(run));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_preserves_dynamic_and_literal_sticks() {
        let rows = messages(Ustr {
            strings: vec![String::new(), "%move, %look / %left-stick.".into()],
        });
        assert!(rows[0].parts.is_empty());
        assert_eq!(
            rows[1].parts,
            [
                MessagePart::Glyph(30),
                MessagePart::Text(", ".into()),
                MessagePart::Glyph(31),
                MessagePart::Text(" / ".into()),
                MessagePart::Glyph(16),
                MessagePart::Text(".".into()),
            ]
        );
    }
}
