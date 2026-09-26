use crate::Event;

pub const MENU_HEIGHT: f32 = 24.0;
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}
/// A full-width time interval with a constant set of simultaneous events.
#[derive(Clone, Debug)]
pub struct Block {
    pub events: Vec<usize>,
    pub rect: Rect,
}
pub fn y_at(minute: f32, start: i32, end: i32, height: f32) -> f32 {
    ((minute - start as f32) / (end - start) as f32).clamp(0.0, 1.0) * height
}

/// Split at event boundaries, never into horizontal lanes. Keeping all event IDs
/// makes every overlapping event available to hover and click interactions.
pub fn layout(events: &[Event], start: i32, end: i32, width: f32, height: f32) -> Vec<Block> {
    let mut visible: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.start < end && e.end > start && e.end > e.start)
        .collect();
    visible.sort_by_key(|(index, e)| (e.start, e.end, *index));
    let mut boundaries: Vec<_> = visible
        .iter()
        .flat_map(|(_, e)| [e.start.max(start), e.end.min(end)])
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
        .windows(2)
        .filter_map(|pair| {
            let ids: Vec<_> = visible
                .iter()
                .filter(|(_, e)| e.start < pair[1] && e.end > pair[0])
                .map(|(index, _)| *index)
                .collect();
            if ids.is_empty() {
                return None;
            }
            let top = y_at(pair[0] as f32, start, end, height);
            let bottom = y_at(pair[1] as f32, start, end, height);
            Some(Block {
                events: ids,
                rect: Rect {
                    x: 1.0,
                    y: top,
                    width: width - 2.0,
                    height: bottom - top,
                },
            })
        })
        .collect()
}
pub fn hit_test(blocks: &[Block], x: f32, y: f32) -> Vec<usize> {
    blocks
        .iter()
        .find(|b| b.rect.contains(x, y))
        .map(|b| b.events.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(start: i32, end: i32) -> Event {
        Event {
            start,
            end,
            title: "test".into(),
            calendar: "test".into(),
            meeting: None,
            color: None,
        }
    }
    #[test]
    fn overlaps_keep_full_width_and_all_event_ids() {
        let blocks = layout(
            &[
                event(500, 600),
                event(550, 610),
                event(580, 590),
                event(610, 650),
            ],
            360,
            1440,
            12.0,
            1080.0,
        );
        assert!(blocks
            .iter()
            .all(|b| b.rect.x == 1.0 && b.rect.width == 10.0));
        assert_eq!(
            blocks.iter().map(|b| b.events.len()).collect::<Vec<_>>(),
            [1, 2, 3, 2, 1, 1]
        );
        assert_eq!(hit_test(&blocks, 2.0, 225.0), vec![0, 1, 2]);
        assert_eq!(hit_test(&blocks, 10.0, 225.0), vec![0, 1, 2]);
        // Touching events are not overlaps: 610 is the end of event 1.
        assert_eq!(hit_test(&blocks, 5.0, 250.0), vec![3]);
    }
    #[test]
    fn clips_crossing_events_and_omits_outside() {
        let blocks = layout(
            &[event(300, 390), event(1410, 1470), event(100, 200)],
            360,
            1440,
            12.0,
            1080.0,
        );
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].rect.y, 0.0);
        assert_eq!(blocks[0].rect.height, 30.0);
        assert_eq!(blocks[1].rect.y + blocks[1].rect.height, 1080.0);
        assert!(hit_test(&blocks, 5.0, 100.0).is_empty());
    }
    #[test]
    fn connected_events_only_darken_actual_intersections() {
        let blocks = layout(
            &[event(400, 600), event(420, 440), event(450, 470)],
            360,
            1440,
            12.0,
            1080.0,
        );
        assert_eq!(
            blocks.iter().map(|b| b.events.len()).collect::<Vec<_>>(),
            [1, 2, 1, 2, 1]
        );
    }
}
