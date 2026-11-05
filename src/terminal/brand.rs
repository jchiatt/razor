//! A pack's product name and theme colours, kept only as far as the terminal uses them.
use super::text::*;
use crate::protocol::Brand;
use ratatui::{buffer::Buffer, style::Color};

/// The brand as shown: a name of at most 64 columns without controls, and only colours for
/// known theme names in `#rrggbb` form.
pub fn read(mut brand: Brand) -> Brand {
    let name: String = brand
        .name
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect();
    brand.name = clip(&name, 64);
    if brand.name.is_empty() {
        brand.name = Brand::default().name;
    }
    brand
        .colours
        .retain(|key, value| token(key).is_some() && colour(value).is_some());
    brand
}

/// Recolours a drawn frame. Each cell is drawn in the canonical theme and substituted once, so
/// an override equal to another name's colour is not substituted again; the default palette
/// costs nothing per cell.
pub fn paint(brand: &Brand, b: &mut Buffer) {
    if brand.colours.is_empty() {
        return;
    }
    let swaps: Vec<_> = brand
        .colours
        .iter()
        .filter_map(|(key, value)| Some((token(key)?, colour(value)?)))
        .collect();
    let swap = |c: Color| {
        swaps
            .iter()
            .find(|(old, _)| *old == c)
            .map_or(c, |(_, new)| *new)
    };
    for cell in &mut b.content {
        (cell.fg, cell.bg) = (swap(cell.fg), swap(cell.bg));
    }
}

fn colour(value: &str) -> Option<Color> {
    let digits = value
        .strip_prefix('#')
        .filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_hexdigit()))?;
    u32::from_str_radix(digits, 16).ok().map(rgb)
}

// The theme's colour names.
fn token(name: &str) -> Option<Color> {
    Some(match name {
        "glass" => GLASS,
        "bezel" => BEZEL,
        "bezelHi" => BEZEL_HI,
        "recess" => RECESS,
        "strip" => STRIP,
        "agentBg" => AGENT_BG,
        "violet" => VIOLET,
        "violetDim" => VIOLET_DIM,
        "white" => WHITE,
        "data" => DATA,
        "gray" => GRAY,
        "grayDim" => GRAY_DIM,
        "green" => GREEN,
        "cyan" => CYAN,
        "magenta" => MAGENTA,
        "amber" => AMBER,
        "red" => RED,
        "redBg" => RED_BG,
        "cyanBg" => CYAN_BG,
        "youBg" => YOU_BG,
        "chip" => CHIP,
        "button" => BUTTON,
        "addedBg" => ADDED_BG,
        _ => return None,
    })
}
