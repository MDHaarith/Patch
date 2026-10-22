//! "Launch at login" through the XDG autostart spec: a `.desktop` file in
//! `$XDG_CONFIG_HOME/autostart` that GNOME, KDE, XFCE, ... run at login.

use crate::paths::APP_ID;
use std::io;
use std::path::{Path, PathBuf};

/// The installed binary. `/proc/self/exe` points at "… (deleted)" after the
/// binary was replaced by an update while running; the new file at the same
/// path is the one to launch next time.
pub fn current_exe() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let s = exe.to_string_lossy();
    Ok(match s.strip_suffix(" (deleted)") {
        Some(stripped) => PathBuf::from(stripped),
        None => exe,
    })
}

/// Quotes one Exec argument per the Desktop Entry spec: double quotes
/// around anything with reserved characters, backslash before `"`, `` ` ``,
/// `$` and `\` inside them, then the general string escaping that doubles
/// every backslash again. `%` is doubled because it starts a field code.
pub fn exec_arg(arg: &str) -> String {
    const RESERVED: &[char] =
        &[' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`'];
    let arg = arg.replace('%', "%%");
    if !arg.is_empty() && !arg.contains(RESERVED) {
        return arg;
    }
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

pub fn entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Patch\n\
         Comment=Keeps Patch running in the background: reminders, widget and sync hub\n\
         Exec={} --background\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         StartupNotify=false\n\
         X-GNOME-Autostart-enabled=true\n\
         X-GNOME-Autostart-Delay=2\n\
         X-KDE-autostart-after=panel\n",
        exec_arg(&exe.to_string_lossy())
    )
}

pub fn enable(file: &Path, exe: &Path) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    patch_core::doc::write_atomic(file, entry(exe).as_bytes())
}

pub fn disable(file: &Path) -> io::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

pub fn is_enabled(file: &Path) -> bool {
    file.exists()
}

/// The program an Exec line starts, undoing [`exec_arg`]'s quoting.
fn exec_program(entry: &str) -> Option<PathBuf> {
    let exec = entry.lines().find_map(|l| l.strip_prefix("Exec="))?;
    // The string-level escape first, then the Exec quoting.
    let exec = exec.replace("\\\\", "\\");
    let program = match exec.strip_prefix('"') {
        Some(rest) => {
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => out.extend(chars.next()),
                    '"' => break,
                    c => out.push(c),
                }
            }
            out
        }
        None => exec.split(' ').next()?.to_string(),
    };
    Some(PathBuf::from(program.replace("%%", "%")))
}

/// Points an existing entry at `exe` when the program it starts is gone
/// (Patch was moved or reinstalled elsewhere). Leaves a working entry alone,
/// so running a development build doesn't take over the installed one, and
/// never creates one: the user may have removed it on purpose with their
/// desktop's startup settings.
pub fn refresh(file: &Path, exe: &Path) -> io::Result<()> {
    match std::fs::read_to_string(file) {
        Ok(current) if exec_program(&current).is_none_or(|p| !p.exists()) => enable(file, exe),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_stay_unquoted() {
        assert_eq!(exec_arg("/home/me/.local/bin/patch"), "/home/me/.local/bin/patch");
    }

    #[test]
    fn reserved_characters_are_quoted() {
        assert_eq!(exec_arg("/home/me/My Apps/patch"), "\"/home/me/My Apps/patch\"");
        // `$` gets a backslash inside the quotes, which the string escape doubles.
        assert_eq!(exec_arg("/opt/$x/patch"), "\"/opt/\\\\$x/patch\"");
        assert_eq!(exec_arg("/opt/a\\b"), "\"/opt/a\\\\\\\\b\"");
        assert_eq!(exec_arg("/opt/100%/patch"), "/opt/100%%/patch");
    }

    #[test]
    fn exec_program_round_trips() {
        for p in ["/home/me/.local/bin/patch", "/home/me/My Apps/patch", "/opt/$x/a\\b", "/opt/100%/patch"] {
            let text = entry(Path::new(p));
            assert_eq!(exec_program(&text), Some(PathBuf::from(p)), "{text}");
        }
    }

    #[test]
    fn enable_refresh_disable() {
        let dir = std::env::temp_dir().join(format!("patch-autostart-test-{}", std::process::id()));
        let file = dir.join("autostart").join("x.desktop");
        refresh(&file, Path::new("/a/patch")).unwrap();
        assert!(!is_enabled(&file), "refresh never creates the entry");

        enable(&file, Path::new("/a/patch")).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\nExec=/a/patch --background\n"));
        assert!(text.starts_with("[Desktop Entry]\n"));

        // "/a/patch" doesn't exist: the entry follows the new location.
        refresh(&file, Path::new("/b/patch")).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("Exec=/b/patch --background"));

        // A working entry is left alone.
        let real = current_exe().unwrap();
        enable(&file, &real).unwrap();
        refresh(&file, Path::new("/somewhere/else/patch")).unwrap();
        assert_eq!(exec_program(&std::fs::read_to_string(&file).unwrap()), Some(real));

        disable(&file).unwrap();
        disable(&file).unwrap();
        assert!(!is_enabled(&file));
        let _ = std::fs::remove_dir_all(dir);
    }
}
