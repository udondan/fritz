//! Shell completion.
//!
//! The scripts call `fritz` on every completion with the words typed so far
//! in the environment variable [`VAR`], and `fritz` answers from its clap
//! definition. So the scripts stay small, follow every change of the
//! commands, and complete the names of the profiles from the configuration
//! file. Nothing contacts the device.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use clap::ValueEnum;
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use clap_complete::env::{CompleteEnv, Shells};
use fritz_core::config::Config;

/// The environment variable that asks `fritz` for completions instead of
/// running a command.
const VAR: &str = "COMPLETE";

/// How to install the scripts, for the help.
pub(crate) const INSTALL: &str = r#"Install:
  bash:       fritz completion bash > ~/.local/share/bash-completion/completions/fritz
  zsh:        fritz completion zsh > "${fpath[1]}/_fritz"
  fish:       fritz completion fish > ~/.config/fish/completions/fritz.fish
  elvish:     fritz completion elvish >> ~/.config/elvish/rc.elv
  PowerShell: fritz completion powershell >> $PROFILE"#;

/// The program that the scripts complete and call, found on the `PATH`.
const BIN: &str = "fritz";

/// Loaded from `$fpath`, the zsh script is the body of the function `_fritz`,
/// which only registers the completer. This also completes on the first tab.
const ZSH_AUTOLOAD: &str =
    r#"if [[ ${funcstack[1]} == _fritz ]]; then _clap_dynamic_completer_fritz "$@"; fi"#;

/// The shells with completion scripts.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum Shell {
    Bash,
    Elvish,
    Fish,
    Powershell,
    Zsh,
}

impl Shell {
    fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Elvish => "elvish",
            Self::Fish => "fish",
            Self::Powershell => "powershell",
            Self::Zsh => "zsh",
        }
    }
}

/// Answers a completion request of a script and exits, if [`VAR`] is set.
/// Returns otherwise.
pub(crate) fn complete(command: fn() -> clap::Command) {
    CompleteEnv::with_factory(command)
        .var(VAR)
        .bin(BIN)
        .completer(BIN)
        .complete();
}

/// Writes the completion script for the shell.
pub(crate) fn script(shell: Shell, out: &mut impl io::Write) -> io::Result<()> {
    let shells = Shells::builtins();
    let completer = shells
        .completer(shell.name())
        .expect("clap_complete supports every shell of Shell");
    completer.write_registration(VAR, BIN, BIN, BIN, out)?;
    if let Shell::Zsh = shell {
        writeln!(out, "{ZSH_AUTOLOAD}")?;
    }
    Ok(())
}

/// Completes `--profile` with the names of the profiles.
pub(crate) fn profiles() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        let path = config_path(std::env::args_os(), std::env::var_os("FRITZ_CONFIG"));
        path.map_or_else(Vec::new, |path| candidates(&path))
    })
}

/// The names of the profiles in the configuration file, with their hosts.
/// Empty if the file cannot be read.
fn candidates(path: &std::path::Path) -> Vec<CompletionCandidate> {
    let Ok(config) = Config::load(path) else {
        return Vec::new();
    };
    config
        .profiles
        .into_iter()
        .map(|(name, profile)| CompletionCandidate::new(name).help(Some(profile.host.into())))
        .collect()
}

/// The configuration file of the command line being completed: the last
/// `--config`, else the environment variable `FRITZ_CONFIG`, else the usual
/// place.
fn config_path(args: impl IntoIterator<Item = OsString>, env: Option<OsString>) -> Option<PathBuf> {
    let mut path = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--config" {
            // Not the next option, when the value is not typed yet.
            path = args
                .next()
                .filter(|value| !value.to_string_lossy().starts_with('-'));
        } else if let Some(value) = arg.to_str().and_then(|a| a.strip_prefix("--config=")) {
            path = Some(value.into());
        }
    }
    path.or(env)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(Config::default_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(args: &[&str], env: Option<&str>) -> Option<PathBuf> {
        config_path(args.iter().map(OsString::from), env.map(OsString::from))
    }

    #[test]
    fn finds_the_configuration_file() {
        let args = [
            "fritz",
            "--",
            "fritz",
            "--config",
            "a.toml",
            "--profile",
            "",
        ];
        assert_eq!(path(&args, Some("b.toml")), Some("a.toml".into()));
        let args = ["fritz", "--", "fritz", "--config=a.toml", "--profile", ""];
        assert_eq!(path(&args, None), Some("a.toml".into()));
        let args = ["fritz", "--", "fritz", "--profile", ""];
        assert_eq!(path(&args, Some("b.toml")), Some("b.toml".into()));
        assert_eq!(path(&args, None), Config::default_path());
        // `--config` without a value yet.
        let args = ["fritz", "--", "fritz", "--config", "--profile", ""];
        assert_eq!(path(&args, Some("b.toml")), Some("b.toml".into()));
        assert_eq!(path(&args, Some("")), Config::default_path());
    }
}
