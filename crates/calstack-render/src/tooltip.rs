use crate::{Canvas, Palette, TextRenderer, Typography};
use calstack_core::{layout::Rect, time_label, Event};

const PADDING: f32 = 8.0;
const CARD_HEIGHT: f32 = 108.0;
const GAP: f32 = 8.0;
const HEADER: f32 = 36.0;

fn card_rect(index: usize, count: usize, width: u32) -> Rect {
    Rect {
        x: PADDING,
        y: if count > 1 { HEADER } else { 0.0 } + PADDING + index as f32 * (CARD_HEIGHT + GAP),
        width: width as f32 - PADDING * 2.0,
        height: CARD_HEIGHT,
    }
}
pub fn event_popup_height(typography: Typography, count: usize, details: bool) -> u32 {
    (PADDING * 2.0
        + if count > 1 { HEADER } else { 0.0 }
        + count as f32 * CARD_HEIGHT
        + count.saturating_sub(1) as f32 * GAP
        + if details { 26.0 } else { 0.0 })
    .mul_add(typography.layout_scale(), 0.0)
    .ceil() as u32
}
/// The same card geometry is used for rendering and pointer hit testing.
/// Padding, header, and the gutters between cards are deliberately not actions.
pub fn event_card_at(
    typography: Typography,
    count: usize,
    width: u32,
    x: f32,
    y: f32,
) -> Option<usize> {
    let unit = typography.layout_scale();
    (0..count).find(|&index| {
        card_rect(index, count, (width as f32 / unit) as u32).contains(x / unit, y / unit)
    })
}

#[allow(clippy::too_many_arguments)]
pub fn event_popup(
    text: &TextRenderer,
    events: &[&Event],
    details: bool,
    width: u32,
    height: u32,
    scale: u32,
    palette: Palette,
    hovered: Option<usize>,
) -> Canvas {
    let mut canvas = Canvas::new(width, height, scale, palette.background);
    let typography = text.typography;
    let unit = typography.layout_scale();
    // Render in shared design coordinates. Pointer hit testing applies the
    // identical transform, including the gaps between cards.
    canvas.scale *= unit;
    let width = (width as f32 / unit) as u32;
    let height = (height as f32 / unit) as u32;
    // A crisp outside edge and separated cards remain legible in either theme.
    for rect in [
        Rect {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: 1.0,
        },
        Rect {
            x: 0.0,
            y: height as f32 - 1.0,
            width: width as f32,
            height: 1.0,
        },
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: height as f32,
        },
        Rect {
            x: width as f32 - 1.0,
            y: 0.0,
            width: 1.0,
            height: height as f32,
        },
    ] {
        canvas.rect(rect, palette.border, 1.0);
    }
    if events.len() > 1 {
        text.strong(
            &mut canvas,
            &format!("{} overlapping events", events.len()),
            (18.0, 25.0),
            typography.body / unit,
            palette.text,
            width as f32 - 36.0,
        );
    }
    for (index, event) in events.iter().enumerate() {
        let card = card_rect(index, events.len(), width);
        canvas.rect(card, palette.card, 1.0);
        if hovered == Some(index) {
            canvas.rect(card, palette.border, 0.35);
        }
        canvas.rect(
            Rect {
                x: card.x,
                y: card.y,
                width: card.width,
                height: 1.0,
            },
            palette.border,
            0.55,
        );
        canvas.rect(
            Rect {
                x: card.x,
                y: card.y + card.height - 1.0,
                width: card.width,
                height: 1.0,
            },
            palette.border,
            0.55,
        );
        let left = card.x + 14.0;
        let right = card.x + card.width - 14.0;
        canvas.rect(
            Rect {
                x: left,
                y: card.y + 13.0,
                width: 5.0,
                height: 5.0,
            },
            event.color.unwrap_or(palette.event),
            1.0,
        );
        text.text(
            &mut canvas,
            &event.calendar,
            (left + 12.0, card.y + 19.0),
            typography.caption / unit,
            palette.muted,
            card.width - 40.0,
        );
        text.strong(
            &mut canvas,
            &event.title,
            (left, card.y + 43.0),
            typography.title / unit,
            palette.text,
            card.width - 28.0,
        );
        let duration = format!("{} min", event.end - event.start);
        let duration_width = text.width(&duration, typography.small / unit);
        text.text(
            &mut canvas,
            &duration,
            (right - duration_width, card.y + 65.0),
            typography.small / unit,
            palette.muted,
            duration_width + 1.0,
        );
        text.text(
            &mut canvas,
            &format!("{} – {}", time_label(event.start), time_label(event.end)),
            (left, card.y + 65.0),
            typography.subtitle / unit,
            palette.text,
            card.width - 44.0 - duration_width,
        );
        let action = if event.meeting.is_some() {
            "Open sample meeting"
        } else {
            "View event details"
        };
        text.text(
            &mut canvas,
            action,
            (left, card.y + 92.0),
            typography.small / unit,
            if hovered == Some(index) {
                palette.text
            } else {
                palette.muted
            },
            card.width - 50.0,
        );
        text.text(
            &mut canvas,
            "›",
            (right - 6.0, card.y + 93.0),
            typography.icon / unit,
            palette.muted,
            8.0,
        );
    }
    if details {
        text.text(
            &mut canvas,
            "Demo calendar · repeats daily",
            (22.0, height as f32 - 12.0),
            typography.caption / unit,
            palette.muted,
            width as f32 - 44.0,
        );
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn larger_desktop_fonts_scale_cards_and_hit_targets_together() {
        let typography = Typography::from_base(18.0);
        let unit = typography.layout_scale();
        let width = (360.0 * unit).ceil() as u32;
        assert_eq!(
            event_card_at(typography, 3, width, 20.0 * unit, 180.0 * unit),
            Some(1)
        );
        assert_eq!(
            event_card_at(typography, 3, width, 20.0 * unit, 156.0 * unit),
            None
        );
        assert_eq!(
            event_popup_height(typography, 3, false),
            (392.0 * unit).ceil() as u32
        );
    }
    #[test]
    fn event_cards_have_separate_click_targets() {
        assert_eq!(
            event_card_at(Typography::default(), 3, 360, 20.0, 20.0),
            None
        ); // header
        assert_eq!(
            event_card_at(Typography::default(), 3, 360, 20.0, 60.0),
            Some(0)
        );
        assert_eq!(
            event_card_at(Typography::default(), 3, 360, 20.0, 156.0),
            None
        ); // gutter
        assert_eq!(
            event_card_at(Typography::default(), 3, 360, 20.0, 180.0),
            Some(1)
        );
        assert_eq!(
            event_card_at(Typography::default(), 3, 360, 20.0, 300.0),
            Some(2)
        );
        assert_eq!(
            event_card_at(Typography::default(), 3, 360, 3.0, 180.0),
            None
        ); // side padding
        assert_eq!(
            event_card_at(
                Typography::default(),
                1,
                360,
                20.0,
                event_popup_height(Typography::default(), 1, false) as f32 - 3.0
            ),
            None
        );
    }
}
