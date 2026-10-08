//! Bundled typography for macOS. Unlike Linux's live fontconfig/Omarchy
//! lookup, this is fixed at build time: resolving the user's live system
//! font via Core Text is an explicit non-goal for the MVP.
use anyhow::Result;
use calstack_render::{TextRenderer, Typography};

static REGULAR: &[u8] = include_bytes!("../../assets/Inter-Regular.ttf");
static BOLD: &[u8] = include_bytes!("../../assets/Inter-Bold.ttf");

pub fn renderer() -> Result<TextRenderer> {
    let mut renderer =
        TextRenderer::new(REGULAR.to_vec(), BOLD.to_vec()).map_err(anyhow::Error::msg)?;
    renderer.typography = Typography::from_base(13.0);
    Ok(renderer)
}
