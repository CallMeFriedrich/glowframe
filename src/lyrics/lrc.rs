//! LRC parser and line timing (SPEC §6.4, §5.3).

use std::time::Duration;

/// Instrumental gaps longer than this show animated dots (SPEC §5.3).
pub const GAP_THRESHOLD: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub time: Duration,
    /// Empty for instrumental markers.
    pub text: String,
}

/// Parses synced lyrics (`[mm:ss.xx] text`). Lines may carry several
/// timestamps; `[offset:+/-ms]` is applied; other tags are ignored.
pub fn parse(lrc: &str) -> Vec<LyricLine> {
    let mut offset_ms: i64 = 0;
    let mut out = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            rest = stripped[end + 1..].trim_start();
            if let Some(t) = parse_timestamp(tag) {
                times.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset_ms = v.trim().parse().unwrap_or(0);
            }
        }
        let text = rest.trim().to_string();
        for t in times {
            out.push((t, text.clone()));
        }
    }
    // A positive LRC offset means the lyrics are shown earlier.
    let mut lines: Vec<LyricLine> = out
        .into_iter()
        .map(|(ms, text)| LyricLine {
            time: Duration::from_millis((ms - offset_ms).max(0) as u64),
            text,
        })
        .collect();
    lines.sort_by_key(|l| l.time);
    lines
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` -> milliseconds.
fn parse_timestamp(tag: &str) -> Option<i64> {
    let (min, sec) = tag.split_once(':')?;
    let min: i64 = min.trim().parse().ok()?;
    let (s, frac) = match sec.split_once(['.', ':']) {
        Some((s, f)) => (s, f),
        None => (sec, ""),
    };
    let s: i64 = s.trim().parse().ok()?;
    if !(0..60).contains(&s) || min < 0 {
        return None;
    }
    let frac_ms = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().ok()? * 100,
        2 => frac.parse::<i64>().ok()? * 10,
        _ => frac[..3].parse::<i64>().ok()?,
    };
    Some(min * 60_000 + s * 1000 + frac_ms)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineState {
    /// Index of the line being sung (last line with time <= position).
    pub current: Option<usize>,
    /// Instrumental gap: show animated dots instead of a line.
    pub gap: bool,
}

pub fn state_at(lines: &[LyricLine], pos: Duration) -> LineState {
    let idx = lines.partition_point(|l| l.time <= pos);
    let current = idx.checked_sub(1);
    let next_time = lines.get(idx).map(|l| l.time);
    let gap = match current {
        // Intro before the first line.
        None => next_time.is_some_and(|t| t > GAP_THRESHOLD),
        // Empty marker line with a long pause until the next line.
        Some(i) => {
            lines[i].text.is_empty()
                && next_time.is_none_or(|t| t.saturating_sub(lines[i].time) > GAP_THRESHOLD)
        }
    };
    LineState { current, gap }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn parses_formats_and_tags() {
        let lrc = "[ar:Someone]\n[ti:Song]\n[00:01.50]First\n[00:03]Second\n[00:05.123] Third \n[01:02.00][01:30.00]Chorus\nno timestamp\n[00:10.00]";
        let l = parse(lrc);
        assert_eq!(l.len(), 6);
        assert_eq!(
            l[0],
            LyricLine {
                time: ms(1500),
                text: "First".into()
            }
        );
        assert_eq!(l[1].time, ms(3000));
        assert_eq!(
            l[2],
            LyricLine {
                time: ms(5123),
                text: "Third".into()
            }
        );
        assert_eq!(
            l[3],
            LyricLine {
                time: ms(10_000),
                text: String::new()
            }
        );
        assert_eq!(l[4].time, ms(62_000));
        assert_eq!(l[5].time, ms(90_000));
        assert_eq!(l[5].text, "Chorus");
    }

    #[test]
    fn applies_offset_tag() {
        let l = parse("[offset:+500]\n[00:02.00]A\n[00:00.20]B");
        assert_eq!(l[0].time, ms(0));
        assert_eq!(l[1].time, ms(1500));
    }

    #[test]
    fn invalid_timestamps_are_ignored() {
        assert!(parse("[aa:bb]x\n[00:75.00]y").is_empty());
    }

    #[test]
    fn current_line_and_gaps() {
        let l = parse(
            "[00:06.00]One\n[00:08.00]Two\n[00:10.00]\n[00:20.00]Three\n[00:21.00]\n[00:23.00]Four",
        );
        // Intro of 6 s -> dots.
        assert_eq!(
            state_at(&l, ms(1000)),
            LineState {
                current: None,
                gap: true
            }
        );
        assert_eq!(
            state_at(&l, ms(6000)),
            LineState {
                current: Some(0),
                gap: false
            }
        );
        assert_eq!(
            state_at(&l, ms(9000)),
            LineState {
                current: Some(1),
                gap: false
            }
        );
        // Empty line with 10 s pause -> dots.
        assert_eq!(
            state_at(&l, ms(12_000)),
            LineState {
                current: Some(2),
                gap: true
            }
        );
        // Empty line but only 2 s pause -> no dots.
        assert_eq!(
            state_at(&l, ms(21_500)),
            LineState {
                current: Some(4),
                gap: false
            }
        );
        assert_eq!(state_at(&l, ms(99_000)).current, Some(5));
        assert_eq!(
            state_at(&[], ms(0)),
            LineState {
                current: None,
                gap: false
            }
        );
    }

    #[test]
    fn short_intro_has_no_dots() {
        let l = parse("[00:02.00]Hi");
        assert_eq!(
            state_at(&l, ms(500)),
            LineState {
                current: None,
                gap: false
            }
        );
    }
}
