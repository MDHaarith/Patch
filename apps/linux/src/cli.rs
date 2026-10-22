//! Command line of the `patch` binary.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
Patch — calendar, board and todos, synced over Tailscale

Usage: patch [OPTIONS]

Options:
  --background       Start without the main window (tray, widget, sync hub and
                     reminders keep running). Used by \"Launch at login\".
  --quit             Ask the running Patch to save and exit.
  --x11              Use X11 (XWayland on a Wayland session). Wayland doesn't let
                     apps keep a window above or below others or remember where
                     it was; Patch picks X11 on its own when the widget or
                     \"Keep Patch on top\" is on.
  --wayland          Use native Wayland even then.
  --data-dir DIR     Keep data in DIR instead of $XDG_DATA_HOME/patch.
  --screenshot DIR   Render the main window in every look and the widget to PNG
                     files in DIR, then exit (needs a display, e.g. xvfb-run).
  -V, --version      Print the version.
  -h, --help         Print this help.

Launching Patch while it is already running shows its window.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Windowing {
    /// X11 when the widget or pinning is enabled (they need window layers and
    /// positions), the session's native protocol otherwise.
    #[default]
    Auto,
    X11,
    Wayland,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub background: bool,
    pub quit: bool,
    pub windowing: Windowing,
    pub data_dir: Option<PathBuf>,
    pub screenshot: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Run(Args),
    Version,
    Help,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        let (flag, inline) = match text.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(OsString::from(v))),
            _ => (text.to_string(), None),
        };
        let mut value = |name: &str| -> Result<PathBuf, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} needs a directory"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--background" => out.background = true,
            "--quit" => out.quit = true,
            "--x11" => out.windowing = Windowing::X11,
            "--wayland" => out.windowing = Windowing::Wayland,
            "--data-dir" => out.data_dir = Some(value("--data-dir")?),
            "--screenshot" => out.screenshot = Some(value("--screenshot")?),
            _ => return Err(format!("unknown option {text:?}")),
        }
    }
    Ok(Command::Run(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn flags() {
        assert_eq!(p(&[]), Ok(Command::Run(Args::default())));
        assert_eq!(p(&["--version"]), Ok(Command::Version));
        assert_eq!(p(&["--background", "-h"]), Ok(Command::Help));
        let Ok(Command::Run(a)) = p(&["--background", "--x11", "--data-dir", "/d"]) else { panic!() };
        assert!(a.background);
        assert_eq!(a.windowing, Windowing::X11);
        assert_eq!(a.data_dir, Some(PathBuf::from("/d")));
        let Ok(Command::Run(a)) = p(&["--screenshot=out"]) else { panic!() };
        assert_eq!(a.screenshot, Some(PathBuf::from("out")));
    }

    #[test]
    fn errors() {
        assert!(p(&["--screenshot"]).is_err());
        assert!(p(&["--screenshot="]).is_err());
        assert!(p(&["--frobnicate"]).is_err());
        assert!(p(&["file.txt"]).is_err());
    }
}
