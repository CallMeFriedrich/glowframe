//! Command line flags for development and testing (see CLAUDE.md).

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliArgs {
    /// Start the fullscreen mode immediately.
    pub start: bool,
    /// Quit the whole program after this duration.
    pub exit_after: Option<Duration>,
    /// Use fake Spotify data and a synthetic 120 BPM beat.
    pub demo: bool,
    /// Log filter, e.g. `info`, `debug`, `glowframe=trace`.
    pub log_level: Option<String>,
}

impl CliArgs {
    /// Parses arguments (without the program name). Unknown flags are
    /// collected as warnings instead of aborting the program.
    pub fn parse<I, S>(args: I) -> (Self, Vec<String>)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut out = Self::default();
        let mut warnings = Vec::new();
        let mut it = args.into_iter().map(Into::into);

        while let Some(arg) = it.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (arg.clone(), None),
            };
            match flag.as_str() {
                "--start" => out.start = true,
                "--demo" => out.demo = true,
                "--exit-after" => match inline.or_else(|| it.next()) {
                    Some(v) => match v.trim().parse::<f64>() {
                        Ok(secs) if secs.is_finite() && secs > 0.0 => {
                            out.exit_after = Some(Duration::from_secs_f64(secs));
                        }
                        _ => warnings.push(format!("invalid value for --exit-after: {v}")),
                    },
                    None => warnings.push("--exit-after requires a value in seconds".into()),
                },
                "--log-level" => match inline.or_else(|| it.next()) {
                    Some(v) if !v.trim().is_empty() => out.log_level = Some(v.trim().to_string()),
                    _ => warnings.push("--log-level requires a value".into()),
                },
                _ => warnings.push(format!("unknown argument: {arg}")),
            }
        }
        (out, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let (a, w) = CliArgs::parse(Vec::<String>::new());
        assert_eq!(a, CliArgs::default());
        assert!(w.is_empty());
    }

    #[test]
    fn all_flags() {
        let (a, w) = CliArgs::parse([
            "--start",
            "--demo",
            "--exit-after",
            "2.5",
            "--log-level=debug",
        ]);
        assert!(w.is_empty());
        assert!(a.start && a.demo);
        assert_eq!(a.exit_after, Some(Duration::from_millis(2500)));
        assert_eq!(a.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn invalid_values_warn() {
        let (a, w) = CliArgs::parse(["--exit-after", "-3", "--bogus", "--log-level"]);
        assert_eq!(a.exit_after, None);
        assert_eq!(a.log_level, None);
        assert_eq!(w.len(), 3);
    }
}
