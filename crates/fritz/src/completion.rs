//! Shell completion.
//!
//! The scripts call `fritz` on every completion with the words typed so far
//! in the environment variable [`VAR`], and `fritz` answers from its clap
//! definition. So the scripts stay small, follow every change of the
//! commands, and complete the names of the profiles from the configuration
//! file. Nothing contacts the device.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;

use clap::ValueEnum;
use clap::builder::{EnumValueParser, PossibleValue, TypedValueParser};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use clap_complete::env::{CompleteEnv, Shells};
use fritz_core::config::Config;

/// The environment variable that asks `fritz` for completions instead of
/// running a command.
const VAR: &str = "COMPLETE";

/// How to install the scripts, for the help. Each shell loads the script
/// from `fritz` when it starts, so it follows updates of fritz.
pub(crate) const INSTALL: &str = r#"Install, so that the script follows updates of fritz:

bash:
  echo 'eval "$(fritz completion bash)"' \
    > ~/.local/share/bash-completion/completions/fritz

zsh:
  printf '%s\n' '#compdef fritz' 'eval "$(fritz completion zsh)"' \
    > "${fpath[1]}/_fritz"

fish:
  echo 'fritz completion fish | source' \
    > ~/.config/fish/completions/fritz.fish

elvish:
  echo 'eval (fritz completion elvish | slurp)' >> ~/.config/elvish/rc.elv

PowerShell:
  Add-Content $PROFILE 'fritz completion powershell | Out-String | Invoke-Expression'"#;

/// The program that the scripts complete and call, found on the `PATH`.
const BIN: &str = "fritz";

/// Loaded from `$fpath`, the zsh script runs in the function `_fritz`, which
/// only registers the completer. This also completes on the first tab, also
/// when `_fritz` loads the script with `eval` or `source`.
const ZSH_AUTOLOAD: &str =
    r#"if (( ${funcstack[(Ie)_fritz]} )); then _clap_dynamic_completer_fritz "$@"; fi"#;

/// The shells with completion scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Shell {
    Bash,
    Elvish,
    Fish,
    #[value(alias = "pwsh")]
    Powershell,
    Zsh,
}

/// Parses a shell by its name or the path of its program, like `$SHELL`:
/// `/bin/zsh`, `-zsh` of a login shell or `pwsh.exe` work as well.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShellParser;

impl TypedValueParser for ShellParser {
    type Value = Shell;

    fn parse_ref(
        &self,
        command: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Shell, clap::Error> {
        let name = value.to_str().map_or(value, |v| OsStr::new(shell_name(v)));
        EnumValueParser::<Shell>::new().parse_ref(command, arg, name)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(Box::new(
            Shell::value_variants()
                .iter()
                .filter_map(ValueEnum::to_possible_value),
        ))
    }
}

/// The name of a shell in its program path, without the dash of a login
/// shell and without `.exe`.
fn shell_name(value: &str) -> &str {
    let name = value.rsplit(['/', '\\']).next().unwrap_or(value);
    let name = name.trim_start_matches('-');
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
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
    fn parses_shells_by_their_path() {
        let command = clap::Command::new("test");
        let parse = |value: &str| ShellParser.parse_ref(&command, None, OsStr::new(value));
        for (value, shell) in [
            ("zsh", Shell::Zsh),
            ("/bin/zsh", Shell::Zsh),
            ("/opt/homebrew/bin/bash", Shell::Bash),
            ("-zsh", Shell::Zsh),
            ("/usr/local/bin/fish", Shell::Fish),
            ("pwsh", Shell::Powershell),
            (
                "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
                Shell::Powershell,
            ),
            ("elvish", Shell::Elvish),
        ] {
            assert_eq!(parse(value).unwrap(), shell, "{value}");
        }
        assert!(parse("/bin/sh").is_err());
        assert!(parse("").is_err());
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
