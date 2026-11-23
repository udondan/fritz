//! Passwords from a command, like the one of a password manager.
//!
//! The command is a list of arguments and runs without a shell. Its
//! standard input and error are those of `fritz`, so a password manager can
//! ask to be unlocked, and its output is the password. Errors name the
//! command, never its output.

use std::fmt;
use std::io::{self, IsTerminal as _, Read as _};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tr064::SecretString;

/// The environment variable with the password command.
pub const ENV_PASSWORD_COMMAND: &str = "FRITZ_PASSWORD_COMMAND";

/// How long the command may take, enough to unlock a password manager.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The password command is invalid or failed. Never contains its output.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The list of arguments is empty, or the program is.
    #[error("the password command is empty")]
    Empty,
    /// The text is neither a JSON list of strings nor plain words.
    #[error("the password command is not a JSON list of strings")]
    Invalid,
    /// The command could not be started or read.
    #[error("cannot run the password command `{0}`")]
    Run(String, #[source] io::Error),
    /// The command exited with an error.
    #[error("the password command `{0}` failed with {1}")]
    Failed(String, ExitStatus),
    /// The command did not finish in time and was stopped.
    #[error("the password command `{0}` did not finish within {1:?}")]
    Timeout(String, Duration),
    /// The command printed nothing.
    #[error("the password command `{0}` printed no password")]
    NoOutput(String),
    /// The command printed something that is not UTF-8.
    #[error("the password command `{0}` printed no valid UTF-8")]
    NotUtf8(String),
}

/// A command that prints the password: the program and its arguments.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct PasswordCommand(Vec<String>);

impl fmt::Debug for PasswordCommand {
    /// Only the program: the arguments may name where the password is.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PasswordCommand")
            .field(&self.name())
            .finish()
    }
}

impl TryFrom<Vec<String>> for PasswordCommand {
    type Error = Error;

    fn try_from(words: Vec<String>) -> Result<Self, Error> {
        if words.first().is_none_or(String::is_empty) {
            return Err(Error::Empty);
        }
        Ok(Self(words))
    }
}

impl From<PasswordCommand> for Vec<String> {
    fn from(command: PasswordCommand) -> Self {
        command.0
    }
}

impl FromStr for PasswordCommand {
    type Err = Error;

    /// A JSON list like `["op", "read", "op://Private/My Box/password"]`, or
    /// words separated by white space, like `pass show fritz`.
    fn from_str(text: &str) -> Result<Self, Error> {
        let words: Vec<String> = if text.trim_start().starts_with('[') {
            serde_json::from_str(text).map_err(|_| Error::Invalid)?
        } else {
            text.split_whitespace().map(str::to_owned).collect()
        };
        words.try_into()
    }
}

impl PasswordCommand {
    /// The program, which errors and `fritz auth status` name.
    pub fn name(&self) -> &str {
        &self.0[0]
    }

    /// Runs the command and returns what it prints, without one trailing
    /// line break. Stops it after `timeout`. Its standard input is the
    /// terminal, if there is one, and else empty, so that it cannot read
    /// what is meant for `fritz`, like the messages of an MCP client.
    ///
    /// Without a terminal, the command gets a process group of its own on
    /// Unix, so that a timeout also stops the processes it started. With a
    /// terminal it stays in the group of `fritz`: a process group in the
    /// background may not read from the terminal, so a password manager
    /// could not ask to be unlocked.
    pub fn run(&self, timeout: Duration) -> Result<SecretString, Error> {
        self.run_with(timeout, io::stdin().is_terminal())
    }

    fn run_with(&self, timeout: Duration, terminal: bool) -> Result<SecretString, Error> {
        let name = || self.name().to_owned();
        let deadline = Instant::now() + timeout;
        let stdin = terminal.then(Stdio::inherit).unwrap_or_else(Stdio::null);
        let mut command = Command::new(&self.0[0]);
        command
            .args(&self.0[1..])
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        if !terminal {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|error| Error::Run(name(), error))?;
        let mut stdout = child.stdout.take().expect("the output is piped");
        let (sender, receiver) = mpsc::channel();
        // A thread, so that a command which prints a lot does not block.
        thread::spawn(move || {
            let mut output = Vec::new();
            let result = stdout.read_to_end(&mut output).map(|_| output);
            // The receiver is gone after a timeout.
            let _ = sender.send(result);
        });
        let timed_out = |mut child: Child| {
            // The group ends the processes the command started, which also
            // closes the output, so that the thread ends. The group is gone
            // if they all exited in the meantime.
            #[cfg(unix)]
            if !terminal {
                use rustix::process::{Pid, Signal, kill_process_group};
                let _ = kill_process_group(Pid::from_child(&child), Signal::KILL);
            }
            // It may have exited in the meantime.
            let _ = child.kill();
            let _ = child.wait();
            Error::Timeout(name(), timeout)
        };
        let Some(status) = wait(&mut child, deadline).map_err(|e| Error::Run(name(), e))? else {
            return Err(timed_out(child));
        };
        if !status.success() {
            return Err(Error::Failed(name(), status));
        }
        // A child of the command may still hold the output open.
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(output) = receiver.recv_timeout(remaining) else {
            return Err(timed_out(child));
        };
        let mut output = output.map_err(|e| Error::Run(name(), e))?;
        if output.last() == Some(&b'\n') {
            output.pop();
            if output.last() == Some(&b'\r') {
                output.pop();
            }
        }
        if output.is_empty() {
            return Err(Error::NoOutput(name()));
        }
        let password = String::from_utf8(output).map_err(|_| Error::NotUtf8(name()))?;
        Ok(password.into())
    }
}

/// Waits for the child to exit. `None` if it is still running at the
/// deadline.
fn wait(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret as _;

    use super::*;

    fn command(words: &[&str]) -> PasswordCommand {
        words
            .iter()
            .map(|w| (*w).to_owned())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap()
    }

    #[test]
    fn parses_lists_and_words() {
        let parsed: PasswordCommand = r#" ["op", "read", "op://Private/My Box/password"]"#
            .parse()
            .unwrap();
        assert_eq!(
            Vec::from(parsed.clone()),
            ["op", "read", "op://Private/My Box/password"]
        );
        assert_eq!(parsed.name(), "op");
        assert_eq!(format!("{parsed:?}"), "PasswordCommand(\"op\")");

        let parsed: PasswordCommand = "pass  show\tfritz\n".parse().unwrap();
        assert_eq!(parsed, command(&["pass", "show", "fritz"]));

        for text in ["", "  ", "[]", r#"[""]"#] {
            let error = text.parse::<PasswordCommand>().unwrap_err();
            assert_eq!(error.to_string(), "the password command is empty", "{text}");
        }
        for text in ["[1]", "[\"op\""] {
            let error = text.parse::<PasswordCommand>().unwrap_err();
            assert_eq!(
                error.to_string(),
                "the password command is not a JSON list of strings"
            );
        }
    }

    #[test]
    fn reports_a_missing_program() {
        let error = command(&["fritz-test-no-such-program"])
            .run(TIMEOUT)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "cannot run the password command `fritz-test-no-such-program`"
        );
    }

    #[cfg(unix)]
    mod unix {
        use super::*;

        const SHORT: Duration = Duration::from_millis(300);

        #[test]
        fn reads_the_password() {
            for (output, expected) in [
                ("secret\n", "secret"),
                ("secret\r\n", "secret"),
                ("secret\n\n", "secret\n"),
                ("secret", "secret"),
                (" with space ", " with space "),
            ] {
                let password = command(&["printf", "%s", output]).run(TIMEOUT).unwrap();
                assert_eq!(password.expose_secret(), expected, "{output:?}");
            }
        }

        #[test]
        fn runs_no_shell() {
            let password = command(&["printf", "%s", "$(echo x) | y; z"])
                .run(TIMEOUT)
                .unwrap();
            assert_eq!(password.expose_secret(), "$(echo x) | y; z");
        }

        #[test]
        fn reports_failures_without_the_output() {
            let error = command(&["sh", "-c", "echo secret; exit 3"])
                .run(TIMEOUT)
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "the password command `sh` failed with exit status: 3"
            );

            for output in ["", "\n"] {
                let error = command(&["printf", output]).run(TIMEOUT).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    "the password command `printf` printed no password"
                );
            }

            let error = command(&["printf", "\\377"]).run(TIMEOUT).unwrap_err();
            assert_eq!(
                error.to_string(),
                "the password command `printf` printed no valid UTF-8"
            );
        }

        #[test]
        fn stops_slow_commands() {
            for terminal in [false, true] {
                let started = Instant::now();
                let error = command(&["sleep", "5"])
                    .run_with(SHORT, terminal)
                    .unwrap_err();
                assert_eq!(
                    error.to_string(),
                    "the password command `sleep` did not finish within 300ms"
                );
                assert!(started.elapsed() < Duration::from_secs(4));
            }
        }

        /// Whether the process is still there, waiting up to 5 seconds for
        /// it to go away.
        fn alive(pid: &str) -> bool {
            (0..100).all(|_| {
                thread::sleep(Duration::from_millis(50));
                let status = Command::new("kill")
                    .args(["-0", pid])
                    .stderr(Stdio::null())
                    .status()
                    .unwrap();
                status.success()
            })
        }

        #[test]
        fn stops_the_processes_the_command_started() {
            // The command exits, but a process it started keeps the output
            // open.
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("pid");
            let script = format!("sleep 30 & echo $! > '{}'; echo secret", file.display());
            let started = Instant::now();
            let error = command(&["sh", "-c", &script])
                .run_with(SHORT, false)
                .unwrap_err();
            assert!(matches!(error, Error::Timeout(..)), "{error}");
            assert!(started.elapsed() < Duration::from_secs(4));
            let pid = std::fs::read_to_string(&file).unwrap();
            assert!(!alive(pid.trim()), "sleep {pid} still runs");
        }

        #[test]
        fn uses_the_terminal_of_fritz() {
            let password = command(&["printf", "secret"])
                .run_with(TIMEOUT, true)
                .unwrap();
            assert_eq!(password.expose_secret(), "secret");
        }
    }
}
