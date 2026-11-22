//! Code view (SPEC §5.4e): the lyrics typed like code in an editor, in
//! sync with the song, with a camera that follows the cursor. Pure logic;
//! drawing lives in `render::ui`.
//!
//! The whole document is built once per song with a typing time for every
//! character, so the view at any song position is a pure function of that
//! position (pause, seek and offset need no extra state). Only the camera
//! keeps spring state.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::lyrics::LyricLine;
use crate::ui::anim::Spring;
use crate::ui::chain::{keywords_ranked, normalize};
use crate::ui::chat::word_spans;

/// Columns left of the code for the line numbers.
pub const GUTTER: f32 = 6.0;
/// Lines visible while the camera follows the cursor.
pub const FOLLOW_LINES: f32 = 8.0;
/// Line height divided by the monospace character width (approximate;
/// only used to keep the line numbers in view).
pub const COLS_PER_LINE: f32 = 2.6;
const INDENT: &str = "    ";
/// Time between two lines that starts a new section (instrumental).
const BREAK_GAP: f32 = 8.0;
/// Blocks longer than this are split every `SPLIT` lines.
const MAX_BLOCK: usize = 8;
const SPLIT: usize = 4;
/// Share of a repeated block's lines found in an earlier chorus for it to
/// count as a repetition of that chorus.
const REPLAY_OVERLAP: f32 = 0.6;
/// Imports at the top (the song's best keywords).
const MAX_IMPORTS: usize = 3;
/// A section change farther than this (lines) zooms out and flies.
const FLY_DISTANCE: f32 = 12.0;
/// Side view angle of the 3D camera (SPEC §5.4e v1.6).
const YAW: f32 = 40.0 * std::f32::consts::PI / 180.0;
/// Optional view from slightly below or above.
const PITCH: f32 = 10.0 * std::f32::consts::PI / 180.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    Function,
    Str,
    Punct,
    Comment,
    Plain,
}

/// A run of characters of one token kind, typed between `t0` and `t1`
/// (song seconds, linear per character).
#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub kind: Tok,
    /// Start column and length in characters.
    pub col: usize,
    pub len: usize,
    pub t0: f32,
    pub t1: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocLine {
    pub text: String,
    /// In column order; always at least one (blank lines: length 0).
    pub segs: Vec<Seg>,
    pub section: usize,
    /// Columns (start, length) of the sung words of a `sing("…")` line.
    pub words: Vec<(usize, usize)>,
}

impl DocLine {
    /// Song second at which the line appears.
    pub fn start(&self) -> f32 {
        self.segs.first().map_or(0.0, |s| s.t0)
    }

    /// Characters typed at song second `t`.
    pub fn revealed(&self, t: f32) -> usize {
        let mut n = 0;
        for s in &self.segs {
            if t >= s.t1 {
                n = s.col + s.len;
            } else if t > s.t0 {
                let f = (t - s.t0) / (s.t1 - s.t0).max(1e-3);
                n = s.col + ((s.len as f32 * f) as usize).min(s.len);
                break;
            } else {
                break;
            }
        }
        n
    }

    /// Still typing (or just finished) at `t`: the cursor stays solid.
    pub fn typing(&self, t: f32) -> bool {
        self.segs.iter().any(|s| t >= s.t0 && t <= s.t1 + 0.4)
    }
}

/// A sung line of a chorus repetition: start time, doc line and the
/// absolute word spans.
pub type ReplayLine = (f32, usize, Vec<(f32, f32)>);

/// One repetition of a chorus: the camera shows the existing function and
/// its lines light up word by word.
#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    /// Focus window in song seconds.
    pub t0: f32,
    pub t1: f32,
    /// Section of the chorus function.
    pub section: usize,
    /// Sung lines: start time, doc line, absolute word spans.
    pub lines: Vec<ReplayLine>,
}

/// Where the camera looks and which section that is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Focus {
    pub line: usize,
    pub col: f32,
    pub section: usize,
    /// Chorus repetition: highlighted doc line and its sung words.
    pub replay: Option<(usize, f32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeDoc {
    pub id: u64,
    pub file_name: String,
    pub lines: Vec<DocLine>,
    pub replays: Vec<Replay>,
}

/// Lowercase line without punctuation, for repetition checks.
fn norm_line(s: &str) -> String {
    s.split_whitespace()
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lowercase ASCII identifier, `None` if nothing is left.
fn ident(s: &str) -> Option<String> {
    let out: String = s
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'ä' => 'a',
            'ö' => 'o',
            'ü' => 'u',
            'ß' => 's',
            c => c,
        })
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    (!out.is_empty() && !out.starts_with(|c: char| c.is_ascii_digit())).then_some(out)
}

/// File name from the song title: `neon_horizon.rs`.
pub fn file_name(title: &str) -> String {
    let mut out = String::new();
    for w in title.split(|c: char| !c.is_alphanumeric()) {
        if let Some(w) = ident(w) {
            if !out.is_empty() {
                out.push('_');
            }
            out.push_str(&w);
        }
        if out.len() > 28 {
            break;
        }
    }
    if out.is_empty() {
        out.push_str("lyrics");
    }
    out + ".rs"
}

fn secs(d: Duration) -> f32 {
    d.as_secs_f32()
}

struct Block {
    lines: Vec<usize>,
    repeat: bool,
    gap_before: bool,
}

/// Splits the sung lines into sections (SPEC §5.4e): breaks at
/// instrumental gaps and where repeated lines start or stop; long blocks
/// are cut every 4 lines.
fn blocks(lines: &[LyricLine]) -> Vec<Block> {
    let text: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].text.trim().is_empty())
        .collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for &i in &text {
        *counts.entry(norm_line(&lines[i].text)).or_default() += 1;
    }
    let mut repeat: Vec<bool> = text
        .iter()
        .map(|&i| counts[&norm_line(&lines[i].text)] >= 2)
        .collect();
    // A single repeated line inside a verse is no chorus.
    let mut k = 0;
    while k < repeat.len() {
        let mut e = k;
        while e < repeat.len() && repeat[e] == repeat[k] {
            e += 1;
        }
        if repeat[k] && e - k == 1 {
            repeat[k] = false;
        }
        k = e;
    }
    let mut out: Vec<Block> = Vec::new();
    for (n, &i) in text.iter().enumerate() {
        let gap = n > 0 && {
            let p = text[n - 1];
            (p + 1..i).any(|j| lines[j].text.trim().is_empty())
                || secs(lines[i].time) - secs(lines[p].time) > BREAK_GAP
        };
        let new = n == 0 || gap || repeat[n] != repeat[n - 1];
        if new {
            out.push(Block {
                lines: Vec::new(),
                repeat: repeat[n],
                gap_before: gap,
            });
        }
        if let Some(b) = out.last_mut() {
            b.lines.push(i);
        }
    }
    // Cut long blocks.
    let mut cut = Vec::new();
    for b in out {
        if b.lines.len() <= MAX_BLOCK {
            cut.push(b);
            continue;
        }
        for (c, chunk) in b.lines.chunks(SPLIT).enumerate() {
            cut.push(Block {
                lines: chunk.to_vec(),
                repeat: b.repeat,
                gap_before: b.gap_before && c == 0,
            });
        }
    }
    cut
}

struct Chorus {
    section: usize,
    name: String,
    /// Normalized line text -> doc line.
    lines: Vec<(String, usize)>,
}

struct Builder {
    lines: Vec<DocLine>,
    replays: Vec<Replay>,
    sections: usize,
    /// Song second up to which everything is typed.
    clock: f32,
}

impl Builder {
    fn section(&mut self) -> usize {
        self.sections += 1;
        self.sections - 1
    }

    /// Closes an open chorus focus when the next element starts.
    fn start_at(&mut self, t: f32) {
        if let Some(r) = self.replays.last_mut()
            && r.t1 == f32::INFINITY
        {
            r.t1 = t;
        }
    }

    /// Line from parts typed evenly (per character) between t0 and t1.
    fn push(&mut self, section: usize, parts: &[(Tok, &str)], t0: f32, t1: f32) {
        let total: usize = parts.iter().map(|(_, s)| s.chars().count()).sum();
        let mut text = String::new();
        let mut segs = Vec::new();
        let mut col = 0;
        for (kind, s) in parts {
            let len = s.chars().count();
            let a = t0 + (t1 - t0) * col as f32 / total.max(1) as f32;
            let b = t0 + (t1 - t0) * (col + len) as f32 / total.max(1) as f32;
            segs.push(Seg {
                kind: *kind,
                col,
                len,
                t0: a,
                t1: b,
            });
            text.push_str(s);
            col += len;
        }
        if segs.is_empty() {
            segs.push(Seg {
                kind: Tok::Plain,
                col: 0,
                len: 0,
                t0,
                t1: t0,
            });
        }
        self.lines.push(DocLine {
            text,
            segs,
            section,
            words: Vec::new(),
        });
        self.clock = self.clock.max(t1);
    }

    fn blank(&mut self, section: usize, t: f32) {
        self.push(section, &[], t, t);
    }

    /// `    sing("word word …");` typed in sync with the line's words.
    fn sing(&mut self, section: usize, line: &LyricLine, next: Option<Duration>) -> usize {
        let t = secs(line.time);
        let text = line.text.replace('"', "'").replace('\\', "/");
        let dur = next.map(|n| n.saturating_sub(line.time));
        let spans = word_spans(&text, dur);
        let mut segs = Vec::new();
        let mut words = Vec::new();
        let mut s = String::new();
        let mut col = 0;
        let mut add = |s: &mut String, segs: &mut Vec<Seg>, kind, part: &str, t0: f32, t1: f32| {
            let len = part.chars().count();
            segs.push(Seg {
                kind,
                col,
                len,
                t0,
                t1,
            });
            s.push_str(part);
            col += len;
            col - len
        };
        // Prefix right before the first word.
        let p0 = self.clock.max(t - 0.18);
        let p1 = (p0 + 0.08).max(t - 0.02);
        add(&mut s, &mut segs, Tok::Plain, INDENT, p0, p0);
        add(
            &mut s,
            &mut segs,
            Tok::Function,
            "sing",
            p0,
            p0 + (p1 - p0) * 0.6,
        );
        add(
            &mut s,
            &mut segs,
            Tok::Punct,
            "(",
            p0 + (p1 - p0) * 0.6,
            p0 + (p1 - p0) * 0.8,
        );
        add(&mut s, &mut segs, Tok::Str, "\"", p0 + (p1 - p0) * 0.8, p1);
        let mut clock = p1;
        for (j, (w, (a, b))) in text.split_whitespace().zip(spans).enumerate() {
            let part = if j == 0 {
                w.to_string()
            } else {
                format!(" {w}")
            };
            let ws = clock.max(t + a);
            let we = (ws + 0.03).max(t + b);
            let at = add(&mut s, &mut segs, Tok::Str, &part, ws, we);
            let skip = usize::from(j > 0);
            words.push((at + skip, w.chars().count()));
            clock = we;
        }
        add(&mut s, &mut segs, Tok::Str, "\"", clock, clock + 0.05);
        add(
            &mut s,
            &mut segs,
            Tok::Punct,
            ");",
            clock + 0.05,
            clock + 0.14,
        );
        self.clock = clock + 0.14;
        self.lines.push(DocLine {
            text: s,
            segs,
            section,
            words,
        });
        self.lines.len() - 1
    }
}

impl CodeDoc {
    /// Builds the code of a song (SPEC §5.4e).
    pub fn build(id: u64, lines: &[LyricLine], title: &str, artist: &str) -> Self {
        let mut b = Builder {
            lines: Vec::new(),
            replays: Vec::new(),
            sections: 0,
            clock: 0.0,
        };
        let first = lines
            .iter()
            .find(|l| !l.text.trim().is_empty())
            .map_or(2.0, |l| secs(l.time));
        // Preamble: comment with title and artist, imports, blank line.
        let pre = b.section();
        let end = (first - 0.8).clamp(0.3, 1.6);
        let head = format!("// {title} – {artist}");
        b.push(pre, &[(Tok::Comment, &head)], 0.0, end * 0.5);
        let imports: Vec<String> = keywords_ranked(lines)
            .iter()
            .filter_map(|w| ident(w))
            .take(MAX_IMPORTS)
            .collect();
        if !imports.is_empty() {
            let mut parts: Vec<(Tok, &str)> = vec![(Tok::Keyword, "import"), (Tok::Punct, " { ")];
            for (i, w) in imports.iter().enumerate() {
                if i > 0 {
                    parts.push((Tok::Punct, ", "));
                }
                parts.push((Tok::Plain, w));
            }
            parts.extend([
                (Tok::Punct, " } "),
                (Tok::Keyword, "from"),
                (Tok::Punct, " "),
                (Tok::Str, "\"lyrics\""),
                (Tok::Punct, ";"),
            ]);
            b.push(pre, &parts, end * 0.5, end);
        }
        b.blank(pre, end);

        let mut choruses: Vec<Chorus> = Vec::new();
        let mut verses = 0;
        let mut after_call = false;
        let next_time = |i: usize| lines.get(i + 1).map(|n| n.time);
        for (n, block) in blocks(lines).into_iter().enumerate() {
            let t_first = secs(lines[block.lines[0]].time);
            if block.gap_before && n > 0 && b.clock + 1.6 < t_first - 0.8 {
                let note = b.section();
                let t0 = b.clock + 0.8;
                b.start_at(t0);
                if after_call {
                    b.blank(note, t0);
                    after_call = false;
                }
                b.push(note, &[(Tok::Comment, "// ♪ instrumental")], t0, t0 + 0.6);
                b.blank(note, t0 + 0.6);
            }
            let norms: Vec<String> = block
                .lines
                .iter()
                .map(|&i| norm_line(&lines[i].text))
                .collect();
            // Repetition of an earlier chorus?
            let known = block.repeat.then(|| {
                choruses.iter().position(|c| {
                    let hits = norms
                        .iter()
                        .filter(|l| c.lines.iter().any(|(x, _)| x == *l))
                        .count();
                    hits as f32 >= norms.len() as f32 * REPLAY_OVERLAP
                })
            });
            if let Some(Some(ci)) = known {
                let c = &choruses[ci];
                let call = b.section();
                let c0 = b.clock.max(t_first - 0.8);
                b.start_at(c0);
                let name = c.name.clone();
                b.push(
                    call,
                    &[(Tok::Function, &name), (Tok::Punct, "();")],
                    c0,
                    c0 + 0.3,
                );
                after_call = true;
                let entries: Vec<ReplayLine> = block
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(k, &i)| {
                        let doc_line = c
                            .lines
                            .iter()
                            .find(|(x, _)| *x == norms[k])
                            .or_else(|| c.lines.get(k.min(c.lines.len() - 1)))
                            .map_or(0, |(_, d)| *d);
                        let t = secs(lines[i].time);
                        let dur = next_time(i).map(|nt| nt.saturating_sub(lines[i].time));
                        let text = lines[i].text.replace('"', "'");
                        let spans = word_spans(&text, dur)
                            .into_iter()
                            .map(|(a, e)| (t + a, t + e))
                            .collect();
                        (t, doc_line, spans)
                    })
                    .collect();
                // Nothing new is typed while the chorus is sung.
                let sung_end: f32 = entries
                    .iter()
                    .filter_map(|(_, _, spans): &ReplayLine| spans.last().map(|(_, e)| *e))
                    .fold(b.clock, f32::max);
                b.clock = b.clock.max(sung_end + 0.1);
                b.replays.push(Replay {
                    t0: c0 + 0.35,
                    t1: f32::INFINITY,
                    section: c.section,
                    lines: entries,
                });
                continue;
            }
            // New function: chorus (first time) or verse.
            let section = b.section();
            let name = if block.repeat {
                match choruses.len() {
                    0 => "chorus".to_string(),
                    k => format!("chorus_{}", k + 1),
                }
            } else {
                verses += 1;
                format!("verse_{verses}")
            };
            let h0 = b.clock.max(t_first - 0.75);
            b.start_at(h0);
            if after_call {
                b.blank(section, h0);
                after_call = false;
            }
            let h1 = (h0 + 0.12).max(t_first - 0.25);
            b.push(
                section,
                &[
                    (Tok::Keyword, "fn"),
                    (Tok::Punct, " "),
                    (Tok::Function, &name),
                    (Tok::Punct, "() {"),
                ],
                h0,
                h1,
            );
            let mut doc_lines = Vec::new();
            for &i in &block.lines {
                let d = b.sing(section, &lines[i], next_time(i));
                doc_lines.push(d);
            }
            let c0 = b.clock + 0.1;
            b.push(section, &[(Tok::Punct, "}")], c0, c0 + 0.08);
            b.blank(section, c0 + 0.12);
            if block.repeat {
                choruses.push(Chorus {
                    section,
                    name,
                    lines: norms.into_iter().zip(doc_lines).collect(),
                });
            }
        }
        CodeDoc {
            id,
            file_name: file_name(title),
            lines: b.lines,
            replays: b.replays,
        }
    }

    /// Cursor (doc line, column) at song second `t`.
    pub fn cursor(&self, t: f32) -> (usize, usize) {
        let n = self.lines.partition_point(|l| l.start() <= t);
        match n.checked_sub(1) {
            Some(i) => (i, self.lines[i].revealed(t)),
            None => (0, 0),
        }
    }

    /// Lines that exist at `t` (typing has reached them).
    pub fn visible_lines(&self, t: f32) -> usize {
        self.lines.partition_point(|l| l.start() <= t)
    }

    /// Camera focus at `t`: a chorus repetition, otherwise the cursor.
    pub fn focus(&self, t: f32) -> Focus {
        if let Some(r) = self.replays.iter().find(|r| t >= r.t0 && t < r.t1) {
            let k = r.lines.partition_point(|(lt, _, _)| *lt <= t);
            let (_, line, spans) = &r.lines[k.saturating_sub(1)];
            let sung = if k == 0 {
                0.0
            } else {
                spans
                    .iter()
                    .map(|(a, e)| ((t - a) / (e - a).max(1e-3)).clamp(0.0, 1.0))
                    .sum()
            };
            let words = &self.lines[*line].words;
            let col = match words.get((sung as usize).min(words.len().saturating_sub(1))) {
                Some((c, l)) if k > 0 => (c + l) as f32,
                _ => self.lines[*line].text.chars().count() as f32 * 0.3,
            };
            return Focus {
                line: *line,
                col,
                section: r.section,
                replay: (k > 0).then_some((*line, sung)),
            };
        }
        let (line, col) = self.cursor(t);
        Focus {
            line,
            col: col as f32,
            section: self.lines.get(line).map_or(0, |l| l.section),
            replay: None,
        }
    }
}

/// Camera state for the renderer: center in (column, line) and the
/// number of visible lines; `tilt` in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    pub x: f32,
    pub y: f32,
    pub lines: f32,
    pub tilt: f32,
    /// 3D view: rotation about the vertical (yaw) and horizontal (pitch)
    /// axis in radians; 0/0 = frontal.
    pub yaw: f32,
    pub pitch: f32,
}

/// 3D angle of a section, rolled per section (SPEC §5.4e): about every
/// second section is seen from the side (40°, left or right), some of
/// those also slightly from below or above.
pub fn section_angle(doc: u64, section: usize) -> (f32, f32) {
    let mut x = doc ^ (section as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    if section == 0 || x & 1 == 0 {
        return (0.0, 0.0);
    }
    let yaw = if x & 2 == 0 { YAW } else { -YAW };
    let pitch = match (x >> 2) % 5 {
        0 => PITCH,
        1 => -PITCH,
        _ => 0.0,
    };
    (yaw, pitch)
}

/// Follows the focus tightly; on a section change it swipes (near) or
/// zooms out, flies and zooms back in (far).
#[derive(Debug)]
pub struct Camera {
    x: Spring,
    y: Spring,
    zoom: Spring,
    yaw: Spring,
    pitch: Spring,
    doc: Option<u64>,
    section: Option<usize>,
    fly_until: Option<Instant>,
    fly_lines: f32,
    swipe_until: Option<Instant>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            x: Spring::new(0.0, 0.45, 0.95),
            y: Spring::new(0.0, 0.5, 0.9),
            zoom: Spring::new(FOLLOW_LINES, 0.7, 1.0),
            yaw: Spring::new(0.0, 1.3, 1.0),
            pitch: Spring::new(0.0, 1.3, 1.0),
            doc: None,
            section: None,
            fly_until: None,
            fly_lines: FOLLOW_LINES,
            swipe_until: None,
        }
    }
}

impl Camera {
    /// `aspect` = width / height of the code area.
    pub fn update(&mut self, now: Instant, doc: u64, focus: Focus, aspect: f32) -> CameraView {
        let target_x = |lines: f32| {
            let cols = lines * aspect * COLS_PER_LINE;
            // Cursor a little right of center; keep the line numbers in
            // view while lines are short.
            (focus.col - cols * 0.08).max(cols * 0.5 - GUTTER)
        };
        let ty = focus.line as f32;
        if self.doc != Some(doc) {
            self.doc = Some(doc);
            self.section = Some(focus.section);
            self.fly_until = None;
            self.swipe_until = None;
            self.zoom.set(FOLLOW_LINES);
            self.x.set(target_x(FOLLOW_LINES));
            self.y.set(ty);
            let (yaw, pitch) = section_angle(doc, focus.section);
            self.yaw.set(yaw);
            self.pitch.set(pitch);
        } else if self.section != Some(focus.section) {
            self.section = Some(focus.section);
            let dy = (ty - self.y.value()).abs();
            if dy > FLY_DISTANCE {
                self.fly_until = Some(now + Duration::from_millis(750));
                self.fly_lines = (dy + 12.0).max(28.0);
            } else {
                self.swipe_until = Some(now + Duration::from_millis(650));
            }
        }
        let flying = self.fly_until.is_some_and(|t| now < t);
        let swiping = self.swipe_until.is_some_and(|t| now < t);
        if flying {
            self.x.retune(1.1, 1.0);
            self.y.retune(1.1, 1.0);
        } else if swiping {
            self.x.retune(0.32, 0.95);
            self.y.retune(0.32, 0.95);
        } else {
            self.x.retune(0.45, 0.95);
            self.y.retune(0.5, 0.9);
        }
        let lines = self
            .zoom
            .update(now, if flying { self.fly_lines } else { FOLLOW_LINES });
        let tx = target_x(lines);
        let x = self.x.update(now, tx);
        let y = self.y.update(now, ty);
        // Lean into the typing direction while the cursor runs ahead.
        // Only for the small lag of typing, not for big camera moves.
        let lag = tx - x;
        let calm = 1.0 - ((lag.abs() - 4.0) / 4.0).clamp(0.0, 1.0);
        let tilt = if flying || swiping {
            0.0
        } else {
            (lag * 0.004).clamp(-0.016, 0.016) * calm
        };
        let (yaw_t, pitch_t) = self.section.map_or((0.0, 0.0), |s| section_angle(doc, s));
        let yaw = self.yaw.update(now, yaw_t);
        let pitch = self.pitch.update(now, pitch_t);
        CameraView {
            x,
            y,
            lines,
            tilt,
            yaw,
            pitch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(secs: f32, text: &str) -> LyricLine {
        LyricLine {
            time: Duration::from_secs_f32(secs),
            text: text.into(),
        }
    }

    /// Verse, chorus, instrumental, verse, chorus again.
    fn song() -> Vec<LyricLine> {
        vec![
            line(5.0, "Every window glowing blue"),
            line(8.0, "The city hums a quiet tune"),
            line(11.0, "Turn it up, turn it round"),
            line(14.0, "Hear the rhythm in the ground"),
            line(17.0, ""),
            line(30.0, "Back again on empty streets"),
            line(33.0, "Counting lights and heart beats"),
            line(36.0, "Turn it up, turn it round"),
            line(39.0, "Hear the rhythm in the ground"),
            line(42.0, "End"),
        ]
    }

    fn doc() -> CodeDoc {
        CodeDoc::build(1, &song(), "Neon Horizon", "Glowframe Demo")
    }

    fn texts(d: &CodeDoc) -> Vec<&str> {
        d.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn structure_has_functions_and_a_call() {
        let d = doc();
        let t = texts(&d);
        assert_eq!(t[0], "// Neon Horizon – Glowframe Demo");
        assert!(t[1].starts_with("import { "));
        assert!(t.contains(&"fn verse_1() {"));
        assert!(t.contains(&"fn chorus() {"));
        assert!(t.contains(&"fn verse_2() {"));
        assert!(t.contains(&"    sing(\"Every window glowing blue\");"));
        assert!(t.contains(&"// ♪ instrumental"));
        assert!(t.contains(&"chorus();"));
        // The chorus is typed once.
        assert_eq!(
            t.iter().filter(|l| l.contains("Turn it up")).count(),
            1,
            "{t:#?}"
        );
        assert_eq!(d.replays.len(), 1);
    }

    #[test]
    fn typing_is_in_time_order() {
        let d = doc();
        assert!(d.lines.windows(2).all(|w| w[0].start() <= w[1].start()));
        for l in &d.lines {
            assert!(l.segs.windows(2).all(|w| w[0].t1 <= w[1].t0 + 1e-4));
        }
    }

    #[test]
    fn words_are_typed_while_sung() {
        let d = doc();
        let i = d
            .lines
            .iter()
            .position(|l| l.text.contains("Every window"))
            .unwrap();
        let l = &d.lines[i];
        // Nothing of the words before the line starts, all shortly after.
        assert!(l.revealed(4.5) <= "    sing(\"".len());
        assert_eq!(l.revealed(8.0), l.text.chars().count());
        let mid = l.revealed(6.0);
        assert!(mid > 11 && mid < l.text.chars().count(), "{mid}");
        assert_eq!(l.words.len(), 4);
        let (c, n) = l.words[1];
        assert_eq!(&l.text[c..c + n], "window");
    }

    #[test]
    fn cursor_follows_typing() {
        let d = doc();
        let (l1, _) = d.cursor(6.0);
        let (l2, _) = d.cursor(16.0);
        assert!(l2 > l1);
        assert_eq!(d.cursor(0.0).0, 0);
    }

    #[test]
    fn chorus_repetition_focuses_the_function() {
        let d = doc();
        let f = d.focus(37.0);
        let chorus_line = d
            .lines
            .iter()
            .position(|l| l.text.contains("Turn it up"))
            .unwrap();
        assert_eq!(f.line, chorus_line);
        assert!(f.replay.is_some_and(|(_, w)| w > 0.0));
        // The focus stays on the chorus while it is sung.
        let f = d.focus(40.5);
        assert!(f.replay.is_some(), "{f:?}");
        assert!(d.lines.iter().all(|l| l.start() < 36.0 || l.start() > 40.0));
        // Typing focus outside the repetition.
        let f = d.focus(31.0);
        assert!(f.replay.is_none());
        assert!(d.lines[f.line].text.contains("Back again") || f.line > chorus_line);
    }

    #[test]
    fn about_half_the_sections_are_seen_from_the_side() {
        let side = (1..400).filter(|&s| section_angle(7, s).0 != 0.0).count();
        assert!((150..250).contains(&side), "{side}");
        assert_eq!(section_angle(7, 0), (0.0, 0.0));
        assert!((1..400).any(|s| section_angle(7, s).1 != 0.0));
        assert!((1..400).any(|s| section_angle(7, s).0 < 0.0));
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name("Neon Horizon"), "neon_horizon.rs");
        assert_eq!(file_name("Über den Wolken!"), "uber_den_wolken.rs");
        assert_eq!(file_name("???"), "lyrics.rs");
    }

    #[test]
    fn quotes_are_escaped() {
        let d = CodeDoc::build(2, &[line(1.0, "say \"hi\" now")], "x", "y");
        assert!(texts(&d).contains(&"    sing(\"say 'hi' now\");"));
    }

    #[test]
    fn camera_flies_far_and_swipes_near() {
        let d = doc();
        let mut c = Camera::default();
        let t0 = Instant::now();
        let v = c.update(t0, d.id, d.focus(6.0), 2.0);
        assert_eq!(v.lines, FOLLOW_LINES);
        // Section change to the chorus repetition far above: zooms out.
        let f = Focus {
            line: 0,
            col: 4.0,
            section: 99,
            replay: None,
        };
        let mut c2 = Camera::default();
        c2.update(
            t0,
            d.id,
            Focus {
                line: 40,
                section: 98,
                ..f
            },
            2.0,
        );
        let mut max_lines: f32 = 0.0;
        for k in 1..60 {
            let v = c2.update(t0 + Duration::from_millis(k * 25), d.id, f, 2.0);
            max_lines = max_lines.max(v.lines);
        }
        assert!(max_lines > 20.0, "{max_lines}");
        let mut v = c2.update(t0, d.id, f, 2.0);
        for k in 60..200 {
            v = c2.update(t0 + Duration::from_millis(k * 25), d.id, f, 2.0);
        }
        assert!((v.lines - FOLLOW_LINES).abs() < 0.1);
        assert!(v.y.abs() < 0.1);
    }
}
