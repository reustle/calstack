use calstack_core::{
    config::Config,
    layout::{y_at, Block, Rect, MENU_HEIGHT},
    Event,
};
use fontdue::{Font, FontSettings};

mod typography;
pub use typography::Typography;
mod tooltip;
pub use tooltip::{event_card_at, event_popup, event_popup_height};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    pub border: [u8; 3],
    pub card: [u8; 3],
    pub event: [u8; 3],
    pub now: [u8; 3],
}
impl Palette {
    pub fn new(light: bool) -> Self {
        if light {
            Self {
                background: [245, 244, 240],
                text: [35, 40, 48],
                muted: [93, 99, 108],
                border: [214, 214, 210],
                card: [255, 255, 252],
                event: [93, 99, 108],
                now: [255, 59, 48],
            }
        } else {
            Self {
                background: [26, 30, 38],
                text: [231, 234, 240],
                muted: [157, 167, 183],
                border: [58, 67, 82],
                card: [34, 39, 48],
                event: [157, 167, 183],
                now: [255, 69, 58],
            }
        }
    }
}

pub struct Canvas {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}
impl Canvas {
    pub fn new(width: u32, height: u32, scale: u32, background: [u8; 3]) -> Self {
        let mut canvas = Self {
            pixels: vec![0; (width * height * scale * scale * 4) as usize],
            width: width * scale,
            height: height * scale,
            scale: scale as f32,
        };
        for pixel in canvas.pixels.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[background[2], background[1], background[0], 255]);
        }
        canvas
    }
    pub fn rect(&mut self, rect: Rect, rgb: [u8; 3], alpha: f32) {
        let x0 = (rect.x * self.scale).floor().max(0.0) as u32;
        let y0 = (rect.y * self.scale).floor().max(0.0) as u32;
        let x1 = ((rect.x + rect.width) * self.scale).ceil().max(0.0) as u32;
        let y1 = ((rect.y + rect.height) * self.scale).ceil().max(0.0) as u32;
        for y in y0..y1.min(self.height) {
            for x in x0..x1.min(self.width) {
                self.blend(x, y, rgb, alpha);
            }
        }
    }
    fn blend(&mut self, x: u32, y: u32, rgb: [u8; 3], alpha: f32) {
        let i = ((y * self.width + x) * 4) as usize;
        for (offset, value) in [rgb[2], rgb[1], rgb[0]].into_iter().enumerate() {
            self.pixels[i + offset] =
                (self.pixels[i + offset] as f32 * (1.0 - alpha) + value as f32 * alpha) as u8;
        }
    }
}

pub struct TextRenderer {
    pub typography: Typography,
    font: Font,
    bold: Font,
}
impl TextRenderer {
    pub fn new(bytes: Vec<u8>, bold_bytes: Vec<u8>) -> Result<Self, &'static str> {
        Ok(Self {
            typography: Typography::default(),
            font: Font::from_bytes(bytes, FontSettings::default())?,
            bold: Font::from_bytes(bold_bytes, FontSettings::default())?,
        })
    }
    /// Single clipped line; truncation is explicit rather than drawing past the popup.
    pub fn text(
        &self,
        canvas: &mut Canvas,
        text: &str,
        position: (f32, f32),
        size: f32,
        color: [u8; 3],
        max_width: f32,
    ) {
        Self::paint_text(&self.font, canvas, text, position, size, color, max_width);
    }
    pub fn strong(
        &self,
        canvas: &mut Canvas,
        text: &str,
        position: (f32, f32),
        size: f32,
        color: [u8; 3],
        max_width: f32,
    ) {
        Self::paint_text(&self.bold, canvas, text, position, size, color, max_width);
    }
    pub fn width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|ch| self.font.metrics(ch, size).advance_width)
            .sum()
    }
    fn paint_text(
        font: &Font,
        canvas: &mut Canvas,
        text: &str,
        position: (f32, f32),
        size: f32,
        color: [u8; 3],
        max_width: f32,
    ) {
        let (x, baseline) = position;
        let scale = canvas.scale;
        let mut px = x * scale;
        let right = ((x + max_width) * scale).min(canvas.width as f32);
        let ellipsis = font.metrics('…', size * scale).advance_width;
        let chars: Vec<_> = text.chars().collect();
        let total_width: f32 = chars
            .iter()
            .map(|&ch| font.metrics(ch, size * scale).advance_width)
            .sum();
        let needs_truncation = total_width > right - px;
        for (index, &original) in chars.iter().enumerate() {
            let advance = font.metrics(original, size * scale).advance_width;
            let truncated =
                needs_truncation && index + 1 < chars.len() && px + advance + ellipsis > right;
            let ch = if truncated { '…' } else { original };
            let (metrics, bitmap) = font.rasterize(ch, size * scale);
            let left = px.round() as i32 + metrics.xmin;
            let top = (baseline * scale).round() as i32 - metrics.height as i32 - metrics.ymin;
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let dx = left + col as i32;
                    let dy = top + row as i32;
                    if dx >= 0 && dy >= 0 && dx < (right as i32) && dy < (canvas.height as i32) {
                        canvas.blend(
                            dx as u32,
                            dy as u32,
                            color,
                            bitmap[row * metrics.width + col] as f32 / 255.0,
                        );
                    }
                }
            }
            px += metrics.advance_width;
            if truncated {
                break;
            }
        }
    }
}

/// Mix existing theme colors; no unrelated accent is introduced.
pub fn mix(background: [u8; 3], foreground: [u8; 3], alpha: f32) -> [u8; 3] {
    std::array::from_fn(|i| {
        (background[i] as f32 * (1.0 - alpha) + foreground[i] as f32 * alpha).round() as u8
    })
}
pub fn overlap_color(base: [u8; 3], background: [u8; 3], depth: usize) -> [u8; 3] {
    mix(
        base,
        background,
        ((depth.saturating_sub(1)) as f32 * 0.18).min(0.75),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn strip(
    text: &TextRenderer,
    config: &Config,
    events: &[Event],
    blocks: &[Block],
    width: u32,
    height: u32,
    scale: u32,
    now: f32,
    hovered: &[usize],
    palette: Palette,
) -> Canvas {
    let mut c = Canvas::new(width, height, scale, palette.background);
    let (start, end) = config.range();
    let drawable = (height as f32 - MENU_HEIGHT).max(1.0);
    for block in blocks {
        let event = &events[block.events[0]];
        let alpha = if block.events == hovered {
            1.0
        } else if now >= event.end as f32 {
            config.appearance.past_opacity
        } else if now >= event.start as f32 {
            config.appearance.active_opacity
        } else {
            config.appearance.future_opacity
        };
        let color = overlap_color(
            event.color.unwrap_or(palette.event),
            palette.background,
            block.events.len(),
        );
        // One logical pixel shorter than the full block, so adjacent events
        // show a hairline of background between them instead of touching.
        c.rect(
            Rect {
                height: (block.rect.height - 1.0).max(0.0),
                ..block.rect
            },
            color,
            alpha as f32,
        );
    }
    // Hour labels use the theme foreground, including over event fills.
    for hour in ((start + 59) / 60)..=((end - 1) / 60) {
        let y = y_at((hour * 60) as f32, start, end, drawable);
        if y + 9.0 < drawable {
            let label = hour.to_string();
            let size = 6.0_f32.min(width as f32 - 2.0);
            let label_width = text.width(&label, size);
            text.text(
                &mut c,
                &label,
                (width as f32 - label_width - 1.0, y + 7.5),
                size,
                palette.text,
                width as f32 - 1.0,
            );
        }
    }
    if config.appearance.show_now_marker && now >= start as f32 && now < end as f32 {
        let y = y_at(now, start, end, drawable);
        c.rect(
            Rect {
                x: 0.0,
                y: y - 0.75,
                width: width as f32,
                height: 1.5,
            },
            palette.now,
            1.0,
        );
        c.rect(
            Rect {
                x: 0.0,
                y: y - 2.0,
                width: 3.0,
                height: 4.0,
            },
            palette.now,
            1.0,
        );
    }
    let menu_y = height as f32 - MENU_HEIGHT;
    c.rect(
        Rect {
            x: 0.0,
            y: menu_y,
            width: width as f32,
            height: 1.0,
        },
        palette.border,
        1.0,
    );
    for dy in [7.0, 11.0, 15.0] {
        c.rect(
            Rect {
                x: width as f32 / 2.0 - 1.0,
                y: menu_y + dy,
                width: 2.0,
                height: 2.0,
            },
            palette.text,
            0.9,
        );
    }
    c
}

pub fn popup(
    text: &TextRenderer,
    lines: &[String],
    width: u32,
    height: u32,
    scale: u32,
    palette: Palette,
    menu_hover: Option<usize>,
) -> Canvas {
    let unit = text.typography.layout_scale();
    let mut c = Canvas::new(width, height, scale, palette.background);
    c.scale *= unit;
    let width = width as f32 / unit;
    let height = height as f32 / unit;
    c.rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: 1.0,
        },
        palette.border,
        1.0,
    );
    c.rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height,
        },
        palette.border,
        1.0,
    );
    c.rect(
        Rect {
            x: 0.0,
            y: height - 1.0,
            width,
            height: 1.0,
        },
        palette.border,
        1.0,
    );
    for (i, line) in lines.iter().enumerate() {
        if menu_hover == Some(i) {
            c.rect(
                Rect {
                    x: 5.0,
                    y: 8.0 + i as f32 * 26.0,
                    width: width - 10.0,
                    height: 26.0,
                },
                palette.border,
                1.0,
            );
        }
        text.text(
            &mut c,
            line,
            (14.0, 27.0 + i as f32 * 26.0),
            text.typography.body / unit,
            if i == 0 || menu_hover.is_some() {
                palette.text
            } else {
                palette.muted
            },
            width - 28.0,
        );
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlap_shades_get_darker() {
        let one = overlap_color([84, 84, 109], [31, 31, 40], 1);
        let two = overlap_color([84, 84, 109], [31, 31, 40], 2);
        let three = overlap_color([84, 84, 109], [31, 31, 40], 3);
        assert!(one.iter().zip(two).all(|(a, b)| *a > b));
        assert!(two.iter().zip(three).all(|(a, b)| *a > b));
        assert_eq!(two[0], two[1]);
    }
}
