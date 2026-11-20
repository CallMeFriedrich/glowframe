//! Layout of the player and cover views in DIPs (SPEC §5.3, §5.4).
//! Pure functions so hit testing and drawing agree.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn centered(cx: f32, cy: f32, w: f32, h: f32) -> Self {
        Self::new(cx - w / 2.0, cy - h / 2.0, w, h)
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Scales around its own center.
    pub fn scaled(&self, s: f32) -> Self {
        let (cx, cy) = self.center();
        Self::centered(cx, cy, self.w * s, self.h * s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Shuffle,
    Previous,
    PlayPause,
    Next,
    Repeat,
}

impl Button {
    pub const ALL: [Button; 5] = [
        Button::Shuffle,
        Button::Previous,
        Button::PlayPause,
        Button::Next,
        Button::Repeat,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerLayout {
    pub cover: Rect,
    /// Two-line box, text bottom-aligned.
    pub title: Rect,
    pub artist: Rect,
    pub buttons: [(Button, Rect); 5],
    pub title_size: f32,
    pub artist_size: f32,
    pub icon_size: f32,
    pub icon_size_big: f32,
}

impl PlayerLayout {
    pub fn hit(&self, x: f32, y: f32) -> Option<Button> {
        self.buttons.iter().find_map(|(b, r)| {
            // Circular buttons.
            let (cx, cy) = r.center();
            let rad = r.w / 2.0;
            ((x - cx).powi(2) + (y - cy).powi(2) <= rad * rad).then_some(*b)
        })
    }
}

/// Centered player view without lyrics.
pub fn player(w: f32, h: f32) -> PlayerLayout {
    player_at(w / 2.0, w, h)
}

/// Player block in the left ~40 % column next to the lyrics (SPEC §5.3).
pub fn player_split(w: f32, h: f32) -> PlayerLayout {
    player_at(w * 0.21, w * 0.4, h)
}

/// Area of the lyrics in the split layout (right ~60 %).
pub fn lyrics_area(w: f32, h: f32) -> Rect {
    Rect::new(w * 0.44, h * 0.08, w * 0.5, h * 0.84)
}

/// Player layout between centered (`t` = 0) and split (`t` = 1).
pub fn player_mixed(w: f32, h: f32, t: f32) -> PlayerLayout {
    if t <= 0.0 {
        return player(w, h);
    }
    let (a, b) = (player(w, h), player_split(w, h));
    if t >= 1.0 {
        return b;
    }
    let lr = |x: Rect, y: Rect| Rect {
        x: x.x + (y.x - x.x) * t,
        y: x.y + (y.y - x.y) * t,
        w: x.w + (y.w - x.w) * t,
        h: x.h + (y.h - x.h) * t,
    };
    let mut buttons = a.buttons;
    for (dst, (_, r)) in buttons.iter_mut().zip(b.buttons.iter()) {
        dst.1 = lr(dst.1, *r);
    }
    PlayerLayout {
        cover: lr(a.cover, b.cover),
        title: lr(a.title, b.title),
        artist: lr(a.artist, b.artist),
        buttons,
        ..a
    }
}

/// Player block centered at `cx` within a column of width `col_w`.
fn player_at(cx: f32, col_w: f32, h: f32) -> PlayerLayout {
    let w = col_w;
    let cover = (0.30 * h).min(0.55 * w);
    let title_size = (0.026 * h).clamp(16.0, 56.0);
    let artist_size = title_size * 0.7;
    let title_h = title_size * 1.25 * 2.0;
    let artist_h = artist_size * 1.45;
    let small = (0.046 * h).clamp(32.0, 96.0);
    let big = small * 1.35;
    let gap_cover = 0.032 * h;
    let gap_text = 0.004 * h;
    let gap_buttons = 0.028 * h;

    let total = cover + gap_cover + title_h + gap_text + artist_h + gap_buttons + big;
    let top = (h - total) / 2.0;
    let text_w = (cover * 1.6).min(w * 0.8);

    let cover_r = Rect::new(cx - cover / 2.0, top, cover, cover);
    let title_y = top + cover + gap_cover;
    let title = Rect::new(cx - text_w / 2.0, title_y, text_w, title_h);
    let artist_y = title_y + title_h + gap_text;
    let artist = Rect::new(cx - text_w / 2.0, artist_y, text_w, artist_h);

    let row_cy = artist_y + artist_h + gap_buttons + big / 2.0;
    let gap = small * 0.42;
    let widths = [small, small, big, small, small];
    let total_w: f32 = widths.iter().sum::<f32>() + gap * 4.0;
    let mut x = cx - total_w / 2.0;
    let mut buttons = [(Button::Shuffle, Rect::default()); 5];
    for (i, b) in Button::ALL.iter().enumerate() {
        let d = widths[i];
        buttons[i] = (*b, Rect::centered(x + d / 2.0, row_cy, d, d));
        x += d + gap;
    }

    PlayerLayout {
        cover: cover_r,
        title,
        artist,
        buttons,
        title_size,
        artist_size,
        icon_size: small * 0.38,
        icon_size_big: big * 0.4,
    }
}

/// Large centered cover (~70 % of the screen height). With lyrics shown
/// below (`lyrics` = 0..1) it moves up and shrinks slightly.
pub fn cover_view(w: f32, h: f32, lyrics: f32) -> Rect {
    let size = ((0.70 - 0.06 * lyrics) * h).min(0.9 * w);
    Rect::centered(w / 2.0, h * (0.5 - 0.06 * lyrics), size, size)
}

/// Font sizes of the lyrics: split view, cover view current/next line.
pub fn lyrics_sizes(h: f32) -> (f32, f32, f32) {
    let big = (0.04 * h).clamp(22.0, 80.0);
    let cover = (0.032 * h).clamp(18.0, 64.0);
    (big, cover, cover * 0.72)
}

/// Font sizes for the standby screen.
pub fn standby_sizes(h: f32) -> (f32, f32) {
    let big = (0.03 * h).clamp(18.0, 64.0);
    (big, big * 0.55)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_is_centered_and_fits() {
        for (w, h) in [
            (1920.0, 1080.0),
            (2560.0, 1440.0),
            (1280.0, 1024.0),
            (1080.0, 1920.0),
        ] {
            let l = player(w, h);
            assert!((l.cover.center().0 - w / 2.0).abs() < 0.01);
            let bottom = l.buttons[2].1.y + l.buttons[2].1.h;
            assert!(l.cover.y > 0.0 && bottom < h, "{w}x{h}");
            // Vertically centered block.
            assert!((l.cover.y - (h - bottom)).abs() < 1.0);
            // Buttons ordered left to right without overlap.
            for pair in l.buttons.windows(2) {
                assert!(pair[0].1.x + pair[0].1.w < pair[1].1.x);
            }
        }
    }

    #[test]
    fn split_layout_stays_left_of_lyrics() {
        let (w, h) = (1920.0, 1080.0);
        let s = player_split(w, h);
        let area = lyrics_area(w, h);
        let right = s.buttons[4].1.x + s.buttons[4].1.w;
        assert!(right < area.x && s.title.x + s.title.w < area.x);
        assert_eq!(player_mixed(w, h, 0.0), player(w, h));
        assert_eq!(player_mixed(w, h, 1.0), s);
        let mid = player_mixed(w, h, 0.5);
        assert!(mid.cover.x > s.cover.x && mid.cover.x < player(w, h).cover.x);
    }

    #[test]
    fn hit_test_buttons() {
        let l = player(1920.0, 1080.0);
        let (cx, cy) = l.buttons[2].1.center();
        assert_eq!(l.hit(cx, cy), Some(Button::PlayPause));
        let r = l.buttons[0].1;
        assert_eq!(
            l.hit(r.x + r.w / 2.0, r.y + r.h / 2.0),
            Some(Button::Shuffle)
        );
        // Corner of the bounding box is outside the circle.
        assert_eq!(l.hit(r.x + 1.0, r.y + 1.0), None);
        assert_eq!(l.hit(10.0, 10.0), None);
    }

    #[test]
    fn cover_view_size() {
        let r = cover_view(1920.0, 1080.0, 0.0);
        assert!((r.h - 756.0).abs() < 0.01);
        assert_eq!(r.center(), (960.0, 540.0));
    }
}
