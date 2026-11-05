//! Command line of the `patch` binary.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
Patch — calendar, board and todos, synced over Tailscale

Usage: patch [OPTIONS]
       patch new                             Open a new task; bind this to a keyboard shortcut
       patch [--data-dir DIR] add TEXT...    Add a task, e.g. patch add Dentist tomorrow 3pm #work
       patch [--data-dir DIR] import FILE    Add an .ics calendar file's events as tasks

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
    pub new_task: bool,
    pub windowing: Windowing,
    pub data_dir: Option<PathBuf>,
    pub screenshot: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Run(Args),
    Add { data_dir: Option<PathBuf>, text: String },
    Import { data_dir: Option<PathBuf>, file: PathBuf },
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
            "new" => out.new_task = true,
            "--x11" => out.windowing = Windowing::X11,
            "--wayland" => out.windowing = Windowing::Wayland,
            "--data-dir" => out.data_dir = Some(value("--data-dir")?),
            "--screenshot" => out.screenshot = Some(value("--screenshot")?),
            "add" | "import" => {
                let rest: Vec<OsString> = args.collect();
                if rest.first().is_some_and(|a| a == "-h" || a == "--help") {
                    return Ok(Command::Help);
                }
                let data_dir = out.data_dir;
                if flag == "import" {
                    return match <[OsString; 1]>::try_from(rest) {
                        Ok([file]) => Ok(Command::Import { data_dir, file: file.into() }),
                        Err(_) => Err("import needs exactly one file".into()),
                    };
                }
                let words: Vec<_> = rest.iter().map(|a| a.to_string_lossy()).collect();
                let text = words.join(" ");
                if text.trim().is_empty() {
                    return Err("add needs the task's text".into());
                }
                return Ok(Command::Add { data_dir, text });
            }
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
    fn new_task() {
        let Ok(Command::Run(a)) = p(&["new"]) else { panic!() };
        assert!(a.new_task);
    }

    #[test]
    fn add_and_import() {
        assert_eq!(
            p(&["--data-dir", "/d", "add", "Dentist", "--tomorrow", "!!!"]),
            Ok(Command::Add { data_dir: Some(PathBuf::from("/d")), text: "Dentist --tomorrow !!!".into() })
        );
        assert_eq!(p(&["import", "a b.ics"]), Ok(Command::Import { data_dir: None, file: PathBuf::from("a b.ics") }));
        assert_eq!(p(&["add", "--help"]), Ok(Command::Help));
        assert!(p(&["add"]).is_err());
        assert!(p(&["add", " "]).is_err());
        assert!(p(&["import"]).is_err());
        assert!(p(&["import", "a", "b"]).is_err());
    }

    #[test]
    fn errors() {
        assert!(p(&["--screenshot"]).is_err());
        assert!(p(&["--screenshot="]).is_err());
        assert!(p(&["--frobnicate"]).is_err());
        assert!(p(&["file.txt"]).is_err());
    }
}
