//! The text read in place on the overlay (ADR 031), as data: the words of an OCR result with a box
//! each, which word a pointer position means, the words a drag picks and the text they copy, the
//! line the bar shows, and the timing of the layer's animations. The overlay paints what this
//! module works out.

use std::ops::RangeInclusive;

use laqta_domain::geometry::PhysRect;
use laqta_ports::OcrResult;

/// A rectangle in the pixels of the image that was read: the selection's physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    fn of(rect: PhysRect) -> Self {
        Rect { x: rect.x as f32, y: rect.y as f32, w: rect.width as f32, h: rect.height as f32 }
    }

    /// How far (`x`, `y`) is outside the rect along each axis; zero inside it.
    fn gap(self, x: f32, y: f32) -> (f32, f32) {
        ((self.x - x).max(x - self.right()).max(0.0), (self.y - y).max(y - self.bottom()).max(0.0))
    }
}

/// One piece of text the user can pick: a word, or one character of a script written without
/// spaces.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    /// The line it belongs to, counting the lines that have text.
    pub line: usize,
    pub rect: Rect,
    pub text: String,
    /// A space stands before it in its line.
    pub spaced: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Line {
    /// The line as the recogniser read it, for a copy of the whole line.
    text: String,
    first: usize,
    len: usize,
}

/// Pixels around a word that still count as on it.
const SLOP: f32 = 2.0;
/// How much more a pointer's distance above or below a word counts than its distance beside it,
/// so that a drag past the end of a line stays on that line.
const LINE_PULL: f32 = 4.0;

/// The words of a result, in reading order, each with its box.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    tokens: Vec<Token>,
    lines: Vec<Line>,
}

impl Layout {
    /// Lays out `result`. A line whose engine placed its words keeps their boxes; any other line
    /// is divided along its length by the usual width of each character, which is close for
    /// proportional type and exact enough to pick words from.
    pub fn new(result: &OcrResult) -> Self {
        let mut layout = Layout::default();
        for line in &result.lines {
            if line.text.trim().is_empty() {
                continue;
            }
            let index = layout.lines.len();
            let first = layout.tokens.len();
            let pieces = if line.words.is_empty() {
                divide(Rect::of(line.rect), &line.text, line.vertical)
            } else {
                let mut pieces = Vec::new();
                // What of the line's text follows the words placed so far.
                let mut rest = line.text.as_str();
                for word in &line.words {
                    let mut parts = divide(Rect::of(word.rect), &word.text, line.vertical);
                    // A space separates the words where the line's text has one. A word the text
                    // does not hold is taken to be a word of its own.
                    let spaced = match rest.find(word.text.as_str()) {
                        Some(at) => {
                            let gap = &rest[..at];
                            rest = &rest[at + word.text.len()..];
                            gap.chars().any(char::is_whitespace)
                        }
                        None => true,
                    };
                    if let Some(first) = parts.first_mut() {
                        first.spaced = spaced && !pieces.is_empty();
                    }
                    pieces.extend(parts);
                }
                pieces
            };
            layout.tokens.extend(pieces.into_iter().map(|piece| Token {
                line: index,
                rect: piece.rect,
                text: piece.text,
                spaced: piece.spaced,
            }));
            let len = layout.tokens.len() - first;
            if len > 0 {
                layout.lines.push(Line { text: line.text.trim().to_owned(), first, len });
            }
        }
        layout
    }

    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// The word at (`x`, `y`), with a little slop around it.
    pub fn at(&self, x: f32, y: f32) -> Option<usize> {
        self.tokens.iter().position(|token| {
            let (dx, dy) = token.rect.gap(x, y);
            dx <= SLOP && dy <= SLOP
        })
    }

    /// The word nearest to (`x`, `y`): the one a drag that has left the text still means.
    pub fn nearest(&self, x: f32, y: f32) -> Option<usize> {
        let distance = |token: &Token| {
            let (dx, dy) = token.rect.gap(x, y);
            dx + LINE_PULL * dy
        };
        (0..self.tokens.len()).min_by(|a, b| distance(&self.tokens[*a]).total_cmp(&distance(&self.tokens[*b])))
    }

    /// Every word of the line that holds `token`.
    pub fn line_of(&self, token: usize) -> RangeInclusive<usize> {
        match self.tokens.get(token).and_then(|t| self.lines.get(t.line)) {
            Some(line) => line.first..=line.first + line.len - 1,
            None => token..=token,
        }
    }

    /// One rectangle per line over the picked words, for the selection highlight.
    pub fn bars(&self, picked: &RangeInclusive<usize>) -> Vec<Rect> {
        let mut bars = Vec::new();
        for line in &self.lines {
            let from = (*picked.start()).max(line.first);
            let to = (*picked.end()).min(line.first + line.len - 1);
            if from > to {
                continue;
            }
            let Some(bar) = self.tokens[from..=to].iter().map(|t| t.rect).reduce(|a, b| {
                let (x, y) = (a.x.min(b.x), a.y.min(b.y));
                Rect { x, y, w: a.right().max(b.right()) - x, h: a.bottom().max(b.bottom()) - y }
            }) else {
                continue;
            };
            bars.push(bar);
        }
        bars
    }

    /// The text of the picked words: lines on their own rows, and a line picked whole exactly as it
    /// was read.
    pub fn text(&self, picked: &RangeInclusive<usize>) -> String {
        let mut rows = Vec::new();
        for line in &self.lines {
            let last = line.first + line.len - 1;
            let from = (*picked.start()).max(line.first);
            let to = (*picked.end()).min(last);
            if from > to {
                continue;
            }
            if from == line.first && to == last {
                rows.push(line.text.clone());
                continue;
            }
            let mut row = String::new();
            for token in &self.tokens[from..=to] {
                if token.spaced && !row.is_empty() {
                    row.push(' ');
                }
                row.push_str(&token.text);
            }
            rows.push(row);
        }
        rows.join("\n")
    }

    /// Everything, line by line.
    pub fn all_text(&self) -> String {
        self.lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Where a word sits in the wave that brings the words in: 0 comes first, 1 last. The wave
    /// runs left to right and leans down the page. `size` is the size of the image that was read.
    pub fn along(&self, token: usize, size: (f32, f32)) -> f32 {
        let Some(token) = self.tokens.get(token) else { return 0.0 };
        let x = (token.rect.x + token.rect.w / 2.0) / size.0.max(1.0);
        let y = (token.rect.y + token.rect.h / 2.0) / size.1.max(1.0);
        (0.7 * x + 0.3 * y).clamp(0.0, 1.0)
    }
}

struct Piece {
    rect: Rect,
    text: String,
    spaced: bool,
}

/// Divides `rect` among the words of `text` by the usual width of each character. Text that runs
/// right to left is measured from the right edge. Text the engine read `down` the page, or a tall,
/// narrow box when it does not say, is divided down its height. A character of a script written
/// without spaces is a piece of its own.
fn divide(rect: Rect, text: &str, down: bool) -> Vec<Piece> {
    let total: f32 = text.chars().map(advance).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let vertical = (down || rect.h > 2.0 * rect.w) && text.chars().filter(|c| !c.is_whitespace()).count() > 1;
    let rtl = reads_right_to_left(text);
    let span = |from: f32, to: f32| {
        let (from, to) = (from / total, to / total);
        if vertical {
            Rect { y: rect.y + from * rect.h, h: (to - from) * rect.h, ..rect }
        } else if rtl {
            Rect { x: rect.x + (1.0 - to) * rect.w, w: (to - from) * rect.w, ..rect }
        } else {
            Rect { x: rect.x + from * rect.w, w: (to - from) * rect.w, ..rect }
        }
    };
    let mut pieces: Vec<Piece> = Vec::new();
    // The piece being built: where it starts, its text, and whether a space came before it.
    let mut open: Option<(f32, String, bool)> = None;
    let mut spaced = false;
    let mut at = 0.0;
    for character in text.chars() {
        let width = advance(character);
        if character.is_whitespace() {
            if let Some((from, text, spaced)) = open.take() {
                pieces.push(Piece { rect: span(from, at), text, spaced });
            }
            spaced = true;
            at += width;
            continue;
        }
        let alone = stands_alone(character);
        if alone && let Some((from, text, spaced)) = open.take() {
            pieces.push(Piece { rect: span(from, at), text, spaced });
        }
        match open.as_mut() {
            Some((_, text, _)) => text.push(character),
            // A mark with nothing before it in this piece belongs to the piece just closed.
            None if width == 0.0 && !spaced && !pieces.is_empty() => {
                if let Some(last) = pieces.last_mut() {
                    last.text.push(character);
                }
                continue;
            }
            None => open = Some((at, character.to_string(), std::mem::take(&mut spaced) && !pieces.is_empty())),
        }
        at += width;
        if alone && let Some((from, text, spaced)) = open.take() {
            pieces.push(Piece { rect: span(from, at), text, spaced });
        }
    }
    if let Some((from, text, spaced)) = open.take() {
        pieces.push(Piece { rect: span(from, at), text, spaced });
    }
    pieces
}

/// A character's usual width beside an ordinary lower-case letter's.
fn advance(character: char) -> f32 {
    match character {
        _ if is_mark(character) => 0.0,
        _ if character.is_whitespace() => 0.55,
        _ if stands_alone(character) => 2.0,
        'i' | 'j' | 'l' | 'I' | '!' | '|' | '\'' | '.' | ',' | ':' | ';' | '`' => 0.5,
        'f' | 'r' | 't' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '-' | '/' | '\\' | '*' => 0.7,
        'm' | 'w' | 'M' | 'W' | '@' | '%' => 1.5,
        _ if character.is_uppercase() => 1.25,
        _ => 1.0,
    }
}

/// Combining marks and joiners, which take no width of their own.
fn is_mark(character: char) -> bool {
    matches!(
        character as u32,
        0x0300..=0x036F | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2 | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED | 0x200C..=0x200F | 0xFE00..=0xFE0F
    )
}

/// Chinese and Japanese characters, written without spaces between words: each is picked alone.
fn stands_alone(character: char) -> bool {
    matches!(
        character as u32,
        0x3000..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2A6DF
    )
}

/// Hebrew, Arabic and their neighbours: the line starts at its right edge.
fn reads_right_to_left(text: &str) -> bool {
    text.chars().find(|c| c.is_alphabetic()).is_some_and(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF))
}

/// How far the pointer must travel from a press before it is a drag and not a click.
pub const DRAG_SLOP: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    from: (f32, f32),
    /// The word the drag started on; a drag from empty space takes the nearest word once it moves.
    anchor: Option<usize>,
    moved: bool,
}

/// The words the user has picked from a layout, and the drag that is picking them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pick {
    layout: Layout,
    /// The word the selection started on and the word it reaches; either may come first.
    picked: Option<(usize, usize)>,
    drag: Option<Drag>,
}

impl Pick {
    pub fn new(layout: Layout) -> Self {
        Pick { layout, picked: None, drag: None }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The picked words, first to last in reading order.
    pub fn picked(&self) -> Option<RangeInclusive<usize>> {
        self.picked.map(|(anchor, head)| anchor.min(head)..=anchor.max(head))
    }

    /// How many words are picked.
    pub fn count(&self) -> usize {
        self.picked().map_or(0, |picked| picked.end() - picked.start() + 1)
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A left press at (`x`, `y`). On a word it picks that word, and its whole line on a double
    /// click; on empty space it clears the selection. With `extend` (Shift) it stretches the
    /// selection to the word under or nearest the pointer instead.
    pub fn press(&mut self, x: f32, y: f32, extend: bool, clicks: usize) {
        let on = self.layout.at(x, y);
        if extend
            && let Some((anchor, _)) = self.picked
            && let Some(head) = on.or_else(|| self.layout.nearest(x, y))
        {
            self.picked = Some((anchor, head));
            self.drag = Some(Drag { from: (x, y), anchor: Some(anchor), moved: true });
            return;
        }
        match on {
            Some(token) if clicks >= 2 => {
                let line = self.layout.line_of(token);
                self.picked = Some((*line.start(), *line.end()));
                self.drag = None;
            }
            Some(token) => {
                self.picked = Some((token, token));
                self.drag = Some(Drag { from: (x, y), anchor: Some(token), moved: false });
            }
            None => {
                self.picked = None;
                self.drag = Some(Drag { from: (x, y), anchor: None, moved: false });
            }
        }
    }

    /// The pointer moved to (`x`, `y`) with the button held: the selection reaches the word there,
    /// or the nearest one. Returns true if the selection changed.
    pub fn drag_to(&mut self, x: f32, y: f32) -> bool {
        let Some(drag) = self.drag.as_mut() else { return false };
        if !drag.moved && (x - drag.from.0).abs() < DRAG_SLOP && (y - drag.from.1).abs() < DRAG_SLOP {
            return false;
        }
        drag.moved = true;
        let anchor = match drag.anchor {
            Some(anchor) => anchor,
            None => {
                let Some(anchor) = self.layout.nearest(drag.from.0, drag.from.1) else { return false };
                drag.anchor = Some(anchor);
                anchor
            }
        };
        let Some(head) = self.layout.nearest(x, y) else { return false };
        let next = Some((anchor, head));
        let changed = self.picked != next;
        self.picked = next;
        changed
    }

    /// The button came up. Returns true if a drag was under way.
    pub fn release(&mut self) -> bool {
        self.drag.take().is_some()
    }

    pub fn pick_all(&mut self) {
        self.drag = None;
        self.picked = self.layout.tokens.len().checked_sub(1).map(|last| (0, last));
    }

    /// Drops the selection. Returns false if there was none.
    pub fn clear(&mut self) -> bool {
        self.drag = None;
        self.picked.take().is_some()
    }

    /// The text of the picked words, if any are picked.
    pub fn text(&self) -> Option<String> {
        self.picked().map(|picked| self.layout.text(&picked))
    }
}

/// What the bar under the selection says.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    Reading,
    /// The selection holds no text.
    NoText,
    Failed(String),
    /// The user just copied: this many words, or everything.
    Copied(Option<usize>),
    /// The user's copy did not reach the clipboard.
    CopyFailed,
    /// Words are picked and not copied yet.
    Picked(usize),
    /// Nothing is picked. `copied`: the whole text went to the clipboard when it was read.
    Found {
        words: usize,
        copied: bool,
    },
}

impl Notice {
    /// The line itself and, where there is room for one, what the user can do next.
    pub fn text(&self) -> (String, Option<&'static str>) {
        match self {
            Notice::Reading => ("Reading text".to_owned(), None),
            Notice::NoText => ("No text found here".to_owned(), Some("Try a larger area")),
            Notice::Failed(message) => (format!("Couldn't read the text: {message}"), None),
            Notice::Copied(Some(count)) => (format!("Copied {}", words(*count)), None),
            Notice::Copied(None) => ("Copied all text".to_owned(), None),
            Notice::CopyFailed => ("Couldn't copy to the clipboard".to_owned(), Some("Try again")),
            Notice::Picked(count) => (format!("{} selected", words(*count)), None),
            Notice::Found { copied: true, .. } => ("All text copied".to_owned(), Some("Drag to pick words")),
            Notice::Found { words: count, copied: false } => (format!("{} found", words(*count)), Some("Drag to pick words")),
        }
    }

    /// The text is on the clipboard: the bar says so with a tick.
    pub fn done(&self) -> bool {
        matches!(self, Notice::Copied(_) | Notice::Found { copied: true, .. })
    }
}

fn words(count: usize) -> String {
    format!("{count} word{}", if count == 1 { "" } else { "s" })
}

/// The timing of the layer's animations. Times are milliseconds since the animation began, and
/// every result is a plain number for the view to paint with.
pub mod motion {
    /// One pass of the band of light that crosses the selection while it is read.
    pub const SWEEP_MS: f32 = 1150.0;
    /// The band's width, as a part of the selection's.
    pub const BAND: f32 = 0.45;
    /// From the first word starting to appear to the last one fully shown.
    pub const REVEAL_MS: f32 = 620.0;
    /// How long one word takes to appear.
    pub const WORD_MS: f32 = 220.0;
    /// How long the picked words stay bright after a copy.
    pub const FLASH_MS: f32 = 520.0;
    /// How long the bar says "Copied".
    pub const NOTICE_MS: f32 = 1700.0;
    /// How long the bar and the dimming take to come in.
    pub const ENTER_MS: f32 = 180.0;

    /// Fast at first, then settling.
    pub fn ease_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    /// Slow at both ends.
    pub fn ease_in_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The centre of the band of light across the selection, as a part of its width: it starts
    /// just off the left edge, ends just off the right, and comes round again.
    pub fn sweep(elapsed: f32) -> f32 {
        let t = (elapsed.max(0.0) % SWEEP_MS) / SWEEP_MS;
        ease_in_out(t) * (1.0 + BAND) - BAND / 2.0
    }

    /// How bright the band is at `offset` from its centre: 1 in the middle, 0 at `-1` and `1`.
    pub fn glow(offset: f32) -> f32 {
        ease_in_out(1.0 - offset.abs())
    }

    /// How far a word has come in, from 0 (not yet) to 1 (shown). `along` is its place in the
    /// wave (`Layout::along`): the words light up one after another, left to right.
    pub fn reveal(elapsed: f32, along: f32) -> f32 {
        let starts = along.clamp(0.0, 1.0) * (REVEAL_MS - WORD_MS);
        ease_out((elapsed - starts) / WORD_MS)
    }

    /// The extra brightness of the picked words after a copy: 1 at once, fading to 0.
    pub fn flash(elapsed: f32) -> f32 {
        let left = (1.0 - elapsed.max(0.0) / FLASH_MS).clamp(0.0, 1.0);
        left * left
    }

    /// A slow breath between 0 and 1, for what waits: the icon and the outline while reading.
    pub fn breath(elapsed: f32) -> f32 {
        0.5 - 0.5 * (elapsed.max(0.0) / 900.0 * std::f32::consts::TAU).cos()
    }

    /// How many dots follow "Reading text": none, then one, two and three.
    pub fn dots(elapsed: f32) -> usize {
        (elapsed.max(0.0) / 320.0) as usize % 4
    }
}

#[cfg(test)]
mod tests {
    use laqta_ports::{OcrLine, OcrWord};

    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect { x, y, width, height }
    }

    fn line(text: &str, at: PhysRect) -> OcrLine {
        OcrLine { rect: at, text: text.to_owned(), confidence: 1.0, words: Vec::new(), vertical: false }
    }

    fn word(text: &str, at: PhysRect) -> OcrWord {
        OcrWord { rect: at, text: text.to_owned() }
    }

    /// Two lines, the engine's own word boxes on the first.
    fn page() -> Layout {
        Layout::new(&OcrResult {
            lines: vec![
                OcrLine {
                    words: vec![
                        word("Hello", rect(10, 10, 50, 20)),
                        word("big", rect(70, 10, 30, 20)),
                        word("world", rect(110, 10, 50, 20)),
                    ],
                    ..line("Hello big world", rect(10, 10, 150, 20))
                },
                line("second line here", rect(10, 40, 160, 20)),
            ],
        })
    }

    fn texts(layout: &Layout) -> Vec<&str> {
        layout.tokens().iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn engine_words_keep_their_boxes() {
        let layout = page();
        assert_eq!(texts(&layout), ["Hello", "big", "world", "second", "line", "here"]);
        assert_eq!(layout.tokens()[1].rect, Rect { x: 70.0, y: 10.0, w: 30.0, h: 20.0 });
        assert!(!layout.tokens()[0].spaced && layout.tokens()[1].spaced);
        assert!(!layout.tokens()[3].spaced, "the first word of a line has no space before it");
    }

    #[test]
    fn a_line_without_word_boxes_is_divided_in_order_inside_its_rect() {
        let layout = Layout::new(&OcrResult { lines: vec![line("ill  WWW  mid", rect(100, 50, 300, 24))] });
        let tokens = layout.tokens();
        assert_eq!(texts(&layout), ["ill", "WWW", "mid"]);
        assert_eq!(tokens[0].rect.x, 100.0);
        assert!((tokens[2].rect.right() - 400.0).abs() < 0.01, "the last word ends at the line's right edge");
        assert!(tokens[0].rect.right() <= tokens[1].rect.x && tokens[1].rect.right() <= tokens[2].rect.x);
        assert!(tokens[1].rect.w > 2.0 * tokens[0].rect.w, "wide letters take more of the line than narrow ones");
        assert!(tokens.iter().all(|t| t.rect.y == 50.0 && t.rect.h == 24.0));
    }

    #[test]
    fn blank_lines_are_left_out() {
        let layout = Layout::new(&OcrResult { lines: vec![line("   ", rect(0, 0, 50, 10)), line("text", rect(0, 20, 40, 10))] });
        assert_eq!(texts(&layout), ["text"]);
        assert_eq!(layout.tokens()[0].line, 0);
        assert_eq!(layout.all_text(), "text");
        assert!(Layout::new(&OcrResult::default()).is_empty());
    }

    #[test]
    fn right_to_left_text_starts_at_the_right_edge() {
        let layout = Layout::new(&OcrResult { lines: vec![line("שלום עולם", rect(0, 0, 200, 20))] });
        let tokens = layout.tokens();
        assert_eq!(texts(&layout), ["שלום", "עולם"]);
        assert!((tokens[0].rect.right() - 200.0).abs() < 0.01, "the first word sits at the right");
        assert!(tokens[1].rect.right() <= tokens[0].rect.x);
        assert_eq!(tokens[1].rect.x, 0.0);
    }

    #[test]
    fn characters_of_a_script_without_spaces_are_picked_one_by_one() {
        let layout = Layout::new(&OcrResult { lines: vec![line("日本語 ok", rect(0, 0, 160, 20))] });
        assert_eq!(texts(&layout), ["日", "本", "語", "ok"]);
        let picked = 1..=2;
        assert_eq!(layout.text(&picked), "本語", "no space is put between them");
        assert_eq!(layout.text(&(2..=3)), "語 ok");
    }

    #[test]
    fn a_tall_narrow_line_is_divided_down_its_height() {
        let layout = Layout::new(&OcrResult { lines: vec![line("縦書き", rect(0, 0, 20, 90))] });
        let tokens = layout.tokens();
        assert_eq!(tokens.len(), 3);
        assert!(tokens.iter().all(|t| t.rect.x == 0.0 && t.rect.w == 20.0));
        assert!(tokens[0].rect.bottom() <= tokens[1].rect.y + 0.01 && (tokens[2].rect.bottom() - 90.0).abs() < 0.01);
    }

    #[test]
    fn boxed_words_are_spaced_as_the_line_text_is() {
        // An engine that boxes the parts of a line written without spaces.
        let layout = Layout::new(&OcrResult {
            lines: vec![OcrLine {
                words: vec![word("日本", rect(0, 0, 40, 20)), word("語", rect(40, 0, 20, 20)), word("ok", rect(70, 0, 20, 20))],
                ..line("日本語 ok", rect(0, 0, 90, 20))
            }],
        });
        let texts: Vec<&str> = layout.tokens().iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["日", "本", "語", "ok"]);
        // A part of the line: no space where the line has none, one where it has one.
        assert_eq!(layout.text(&(1..=2)), "本語");
        assert_eq!(layout.text(&(2..=3)), "語 ok");
    }

    #[test]
    fn a_line_the_engine_read_down_the_page_is_divided_down_it() {
        // Short and only a little taller than wide: by its shape alone it would be divided across.
        let down = OcrLine { vertical: true, ..line("縦書", rect(0, 0, 20, 35)) };
        let layout = Layout::new(&OcrResult { lines: vec![down.clone()] });
        assert!(layout.tokens().iter().all(|t| t.rect.x == 0.0 && t.rect.w == 20.0));
        // The same for its one word box, which an engine gives a line written without spaces.
        let boxed = OcrLine { words: vec![word("縦書", rect(0, 0, 20, 35))], ..down };
        let layout = Layout::new(&OcrResult { lines: vec![boxed] });
        assert!(layout.tokens().iter().all(|t| t.rect.x == 0.0 && t.rect.w == 20.0));
    }

    #[test]
    fn a_point_finds_the_word_under_it_or_the_nearest() {
        let layout = page();
        assert_eq!(layout.at(75.0, 15.0), Some(1));
        assert_eq!(layout.at(105.0, 15.0), None, "between two words");
        assert_eq!(layout.nearest(104.0, 15.0), Some(1));
        assert_eq!(layout.nearest(106.0, 15.0), Some(2));
        // Far to the right of the first line: its last word, not a nearer word of the next line.
        assert_eq!(layout.nearest(400.0, 28.0), Some(2));
        assert_eq!(layout.nearest(-50.0, 500.0), Some(3));
        assert_eq!(Layout::default().nearest(0.0, 0.0), None);
    }

    #[test]
    fn a_drag_picks_from_the_first_word_to_the_last_in_reading_order() {
        let mut pick = Pick::new(page());
        pick.press(75.0, 15.0, false, 1);
        assert_eq!(pick.picked(), Some(1..=1), "a press on a word picks it");
        assert!(!pick.drag_to(76.0, 16.0), "not a drag yet");
        assert!(pick.drag_to(100.0, 50.0));
        assert_eq!(pick.picked(), Some(1..=4));
        assert_eq!(pick.text().as_deref(), Some("big world\nsecond line"));
        assert_eq!(pick.count(), 4);
        assert!(!pick.drag_to(101.0, 50.0), "still on the same word");
        // Back above where it started: the selection turns round.
        pick.drag_to(20.0, 12.0);
        assert_eq!(pick.picked(), Some(0..=1));
        assert!(pick.release() && !pick.dragging());
        assert!(!pick.drag_to(150.0, 50.0), "nothing moves the selection once the button is up");
    }

    #[test]
    fn a_drag_from_empty_space_takes_the_nearest_words() {
        let mut pick = Pick::new(page());
        pick.press(300.0, 12.0, false, 1);
        assert_eq!(pick.picked(), None);
        pick.drag_to(300.0, 55.0);
        assert_eq!(pick.picked(), Some(2..=5));
    }

    #[test]
    fn a_press_on_empty_space_clears_and_a_double_click_takes_the_line() {
        let mut pick = Pick::new(page());
        pick.press(20.0, 50.0, false, 2);
        assert_eq!(pick.picked(), Some(3..=5));
        assert_eq!(pick.text().as_deref(), Some("second line here"));
        pick.press(300.0, 300.0, false, 1);
        pick.release();
        assert_eq!(pick.picked(), None);
        assert_eq!(pick.text(), None);
    }

    #[test]
    fn shift_stretches_the_selection_and_all_picks_everything() {
        let mut pick = Pick::new(page());
        pick.press(20.0, 15.0, false, 1);
        pick.release();
        pick.press(130.0, 50.0, true, 1);
        assert_eq!(pick.picked(), Some(0..=5));
        assert!(pick.clear() && !pick.clear());
        pick.pick_all();
        assert_eq!(pick.text().as_deref(), Some("Hello big world\nsecond line here"));
        let mut none = Pick::default();
        none.pick_all();
        assert_eq!(none.picked(), None);
    }

    #[test]
    fn a_whole_line_copies_as_it_was_read_and_bars_cover_each_line() {
        let layout = Layout::new(&OcrResult { lines: vec![line("a  -  b", rect(0, 0, 70, 10)), line("c d", rect(0, 20, 30, 10))] });
        assert_eq!(layout.text(&(0..=4)), "a  -  b\nc d");
        assert_eq!(layout.text(&(1..=3)), "- b\nc");
        let bars = layout.bars(&(1..=3));
        assert_eq!(bars.len(), 2);
        assert_eq!((bars[1].x, bars[1].y), (0.0, 20.0));
        assert!(bars[0].x > 0.0 && (bars[0].right() - 70.0).abs() < 0.01);
        assert_eq!(layout.line_of(4), 3..=4);
    }

    #[test]
    fn the_bar_says_what_happened() {
        assert_eq!(Notice::Copied(Some(1)).text().0, "Copied 1 word");
        assert_eq!(Notice::Picked(12).text().0, "12 words selected");
        assert_eq!(Notice::Found { words: 3, copied: false }.text().0, "3 words found");
        assert!(Notice::Found { words: 3, copied: true }.done() && Notice::Copied(None).done());
        assert!(!Notice::Picked(2).done() && !Notice::Reading.done());
        assert!(Notice::Failed("no model".into()).text().0.ends_with("no model"));
    }

    #[test]
    fn the_wave_brings_words_in_from_the_left() {
        let layout = page();
        let size = (200.0, 80.0);
        assert!(layout.along(0, size) < layout.along(2, size));
        assert_eq!(motion::reveal(0.0, 0.5), 0.0);
        assert!(motion::reveal(60.0, 0.0) > 0.0 && motion::reveal(60.0, 1.0) == 0.0);
        assert_eq!(motion::reveal(motion::REVEAL_MS, 1.0), 1.0);
    }

    #[test]
    fn the_band_crosses_the_selection_and_comes_round_again() {
        assert!(motion::sweep(0.0) < 0.0 && motion::sweep(motion::SWEEP_MS - 1.0) > 1.0);
        assert!((motion::sweep(motion::SWEEP_MS / 2.0) - 0.5).abs() < 0.01);
        assert_eq!(motion::sweep(motion::SWEEP_MS), motion::sweep(0.0));
        assert_eq!((motion::glow(0.0), motion::glow(1.0), motion::glow(-1.5)), (1.0, 0.0, 0.0));
        assert_eq!((motion::flash(0.0), motion::flash(motion::FLASH_MS)), (1.0, 0.0));
        assert_eq!((motion::dots(0.0), motion::dots(330.0), motion::dots(1290.0)), (0, 1, 0));
        assert!(motion::breath(0.0) < 0.01 && motion::breath(450.0) > 0.99);
    }
}
