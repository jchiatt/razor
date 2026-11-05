//! The composer's phone form: a dock row, or the speaker row in a child's split, over the words
//! with the ☺ chip at the right.
use super::super::{
    App,
    text::*,
    view::{Action, Hit},
};
use super::{Composer, emoji_chip};
use ratatui::{buffer::Buffer, layout::Rect};

impl Composer {
    /// The phone form's height: the dock row, then the words' rows, one up to a third of the
    /// screen (three to ten) before they scroll.
    pub fn phone_rows(&self, width: u16, height: u16) -> u16 {
        let cap = (height / 3).clamp(3, 10) as usize;
        1 + self.height(width.saturating_sub(6) as usize, cap).max(1) as u16
    }

    /// The phone form: the dock row (` ◧ status`, and the next input's model, effort and name at
    /// the right) or, while a child is addressed, the speaker row; then ` ❯ ` with the words or
    /// the hint for this width, and the ☺ chip in the last three columns.
    pub fn phone(
        &self,
        app: &App,
        b: &mut Buffer,
        r: Rect,
        hits: &mut Vec<Hit>,
    ) -> Option<(u16, u16)> {
        fill(b, r, style(WHITE, BEZEL));
        if r.width < 8 || r.height < 2 {
            return None;
        }
        if app.split_view() {
            speaker(app, b, r);
        } else {
            let (mark, words, tone) =
                super::super::status::chip(app, r.width.saturating_sub(5) as usize);
            let w = width(&words) as u16 + 3;
            let chip = Rect::new(r.x + 1, r.y, w, 1);
            let ink = style([WHITE, GRAY][usize::from(tone == GRAY)], BEZEL);
            put(b, chip.x, r.y, &format!("{mark} {words} "), w, ink);
            put(b, chip.x, r.y, mark, 1, style(tone, BEZEL));
            hits.push((chip, super::super::status::press(app)));
            let x = chip.right() + 1;
            let rest = r.right().saturating_sub(x + 1);
            identity(app, b, Rect::new(x, r.y, rest, 1));
        }
        let words = Rect::new(r.x + 3, r.y + 1, r.width - 6, r.height - 1);
        let cursor = self.content(b, words, app);
        let above = self.window.borrow().as_ref().is_some_and(|w| w.first > 0);
        put(
            b,
            r.x,
            words.y,
            if above { " ↑ " } else { " ❯ " },
            3,
            style(CYAN, BEZEL),
        );
        let chip = Rect::new(r.right() - 3, words.y, 3, 1);
        put(b, chip.x, chip.y, " ☺ ", 3, emoji_chip(app));
        hits.push((chip, Action::Emoji));
        cursor
    }
}

// A child's split on a phone: whom the words are for and that they go as the person's, with
// the way back at the right.
fn speaker(app: &App, b: &mut Buffer, r: Rect) {
    let agent = app.row();
    let name = agent.map_or("child", |a| a.name.as_str());
    let x = put(b, r.x, r.y, " YOU", r.width, bold(CYAN, BEZEL));
    let long = format!(" · talking to {name}");
    let lead = match width(&long) + 24 <= r.width as usize {
        true => long,
        false => format!(" → {name}"),
    };
    let room = (r.width as usize)
        .saturating_sub(width(" YOU · as you"))
        .max(4);
    let x = put(
        b,
        x,
        r.y,
        &clip(&lead, room),
        r.right() - x,
        bold(AMBER, BEZEL),
    );
    let x = put(b, x, r.y, " · ", r.right() - x, style(GRAY, BEZEL));
    let (voice, ink) = super::speaks_as(agent);
    put(b, x, r.y, voice, r.right() - x, style(ink, BEZEL));
}

// The next input's model, effort and name, right-aligned in `r`: the longest form that fits.
fn identity(app: &App, b: &mut Buffer, r: Rect) {
    let (model, effort) = super::chosen(app);
    let agent = app.row();
    let dot = (" · ", style(GRAY, BEZEL));
    let all = [
        (model.as_str(), bold(CYAN, BEZEL)),
        dot,
        (effort.as_str(), style(CYAN, BEZEL)),
        dot,
        (
            agent.map_or("you", |a| a.name.as_str()),
            style(VIOLET, BEZEL),
        ),
    ];
    for n in [5, 3, 1] {
        let need: usize = all[..n].iter().map(|(words, _)| width(words)).sum();
        if need <= r.width as usize {
            let mut x = r.right() - need as u16;
            for (words, look) in &all[..n] {
                x = put(b, x, r.y, words, r.right() - x, *look);
            }
            return;
        }
    }
}
