//! Where `fritz` keeps its files. On Linux and macOS these are the XDG base
//! directories, like on most command line tools: `~/.config/fritz` and
//! `~/.local/state/fritz`, or below `$XDG_CONFIG_HOME` and
//! `$XDG_STATE_HOME`. On Windows they are the usual ones below `%APPDATA%`
//! and `%LOCALAPPDATA%`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

const NAME: &str = "fritz";

/// The directory of the configuration. `None` if the home directory is
/// unknown.
pub fn config() -> Option<PathBuf> {
    if cfg!(windows) {
        directories::ProjectDirs::from("", "", NAME).map(|dirs| dirs.config_dir().to_owned())
    } else {
        xdg(
            std::env::var_os("XDG_CONFIG_HOME"),
            home().as_deref(),
            ".config",
        )
    }
}

/// The directory of the state, like tokens and the audit log. `None` if the
/// home directory is unknown.
pub fn state() -> Option<PathBuf> {
    if cfg!(windows) {
        directories::ProjectDirs::from("", "", NAME).map(|dirs| dirs.data_local_dir().to_owned())
    } else {
        xdg(
            std::env::var_os("XDG_STATE_HOME"),
            home().as_deref(),
            ".local/state",
        )
    }
}

fn home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_owned())
}

/// The directory of `fritz` below the XDG variable, or below its default in
/// the home directory. The specification ignores relative paths.
fn xdg(variable: Option<OsString>, home: Option<&Path>, default: &str) -> Option<PathBuf> {
    let base = variable
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| home.join(default)))?;
    Some(base.join(NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_variable() {
        assert_eq!(
            xdg(Some("/xdg".into()), Some(Path::new("/home/me")), ".config"),
            Some(PathBuf::from("/xdg/fritz"))
        );
    }

    #[test]
    fn falls_back_to_the_home_directory() {
        let home = Some(Path::new("/home/me"));
        let expected = Some(PathBuf::from("/home/me/.local/state/fritz"));
        assert_eq!(xdg(None, home, ".local/state"), expected);
        assert_eq!(xdg(Some("".into()), home, ".local/state"), expected);
        assert_eq!(xdg(Some("relative".into()), home, ".local/state"), expected);
    }

    #[test]
    fn needs_a_home_directory() {
        assert_eq!(xdg(None, None, ".config"), None);
    }

    #[test]
    fn ends_in_fritz() {
        assert!(config().unwrap().ends_with(NAME));
        assert!(state().unwrap().ends_with(NAME));
    }
}
