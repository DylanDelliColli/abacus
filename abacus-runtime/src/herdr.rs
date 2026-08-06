//! The pinned Herdr provider: real process invocation behind a
//! hermetic seam.
//!
//! This is the first ABACUS code that talks to something outside the
//! workspace. Two rules shape it, and both are contract obligations
//! rather than preferences.
//!
//! **The pin is a gate, not a label.** `.abacus/providers.lock.toml`
//! records `mismatch_policy = "fail-closed"`, so identity is verified
//! against the pin BEFORE any mutating verb and any doubt refuses.
//! Unparseable output is a mismatch, never a guess — a provider whose
//! answer we cannot read is precisely the one not to trust.
//!
//! **Process execution is injected.** [`CommandRunner`] keeps the
//! default test lane hermetic (AGENTS.md: no live provider outside the
//! explicit lane). This module never names `std::process`; the real
//! spawning runner lands with the live lane (`ABACUS-gyh.6`), so no
//! unit test can reach a process even by accident.
//!
//! The pin values arrive as a [`HerdrPin`] value rather than being read
//! from disk here. Parsing the lock file belongs to the composition
//! root, which keeps this module free of file I/O and makes every
//! identity case expressible as an ordinary unit test.

use crate::adapter::RawRunError;
use abacus_core::Timestamp;

/// One completed process invocation, normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Why an invocation produced no usable output. Distinct from a
/// provider that ran and answered unfavorably, which is a
/// [`CommandOutput`] with a non-zero status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandError {
    /// The executable could not be run at all.
    Unavailable,
    /// The host refused execution (sandbox/approval).
    NotPermitted,
    /// The deadline elapsed before the process produced output.
    Timeout,
}

/// The seam that keeps this module testable without a live provider.
pub trait CommandRunner {
    fn run(&self, argv: &[String], deadline: Timestamp) -> Result<CommandOutput, CommandError>;
}

/// The pinned identity every invocation is checked against. Mirrors the
/// `[providers.herdr]` entry in `.abacus/providers.lock.toml`; the
/// composition root parses that file and hands the values here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrPin {
    /// Binary-reported version, exactly as `herdr --version` prints it.
    pub version: String,
    /// Bundled API schema version.
    pub api_schema_version: u32,
    /// Bundled API protocol number. The 17→19 change is what made
    /// v0.8.0 a full pin change rather than an upgrade.
    pub api_protocol: u32,
}

/// The real provider. Generic over its runner so the default test lane
/// never spawns a process.
#[derive(Debug, Clone)]
pub struct HerdrProvider<R> {
    runner: R,
    pin: HerdrPin,
    executable: String,
}

impl<R: CommandRunner> HerdrProvider<R> {
    pub fn new(runner: R, pin: HerdrPin, executable: impl Into<String>) -> Self {
        Self {
            runner,
            pin,
            executable: executable.into(),
        }
    }

    fn argv(&self, args: &[&str]) -> Vec<String> {
        let mut argv = Vec::with_capacity(args.len() + 1);
        argv.push(self.executable.clone());
        argv.extend(args.iter().map(|arg| (*arg).to_owned()));
        argv
    }

    /// Run one invocation, normalizing every failure to the fail-closed
    /// identity verdict. A runner error, a non-zero exit, and garbled
    /// output are all "identity unproven" here; distinguishing them
    /// would invite treating some of them as good enough.
    fn identity_output(&self, args: &[&str], deadline: Timestamp) -> Result<String, RawRunError> {
        match self.runner.run(&self.argv(args), deadline) {
            Ok(output) if output.status == 0 => Ok(output.stdout),
            Ok(_) => Err(RawRunError::VersionMismatch),
            Err(CommandError::NotPermitted) => Err(RawRunError::NotPermitted),
            Err(CommandError::Unavailable) => Err(RawRunError::Unavailable),
            Err(CommandError::Timeout) => Err(RawRunError::VersionMismatch),
        }
    }

    /// Verify the running binary against the pin. Called before any
    /// mutating verb; every uncertain outcome refuses.
    pub fn verify_pinned_identity(&self, deadline: Timestamp) -> Result<(), RawRunError> {
        let version = self.identity_output(&["--version"], deadline)?;
        if parse_version(&version).as_deref() != Some(self.pin.version.as_str()) {
            return Err(RawRunError::VersionMismatch);
        }

        let snapshot = self.identity_output(&["api", "snapshot"], deadline)?;
        let Some((schema_version, protocol)) = parse_snapshot_identity(&snapshot) else {
            // Unreadable output is a mismatch. A provider whose answer
            // we cannot parse is not one to mutate through.
            return Err(RawRunError::VersionMismatch);
        };
        if schema_version != self.pin.api_schema_version || protocol != self.pin.api_protocol {
            return Err(RawRunError::VersionMismatch);
        }
        Ok(())
    }
}

/// `herdr --version` prints a name-and-version line; take the last
/// whitespace-separated token so a bare version or a prefixed one both
/// parse, and reject anything with no token at all.
fn parse_version(raw: &str) -> Option<String> {
    let line = raw.lines().next()?.trim();
    let token = line.split_whitespace().next_back()?;
    let token = token.trim_start_matches('v');
    (!token.is_empty()).then(|| token.to_owned())
}

/// Pull the schema version and protocol out of an `api snapshot`
/// document without taking a JSON dependency for two integers. Both
/// must be present; a document carrying only one is not a partial
/// success.
fn parse_snapshot_identity(raw: &str) -> Option<(u32, u32)> {
    let schema = scan_u32(raw, "\"schema_version\"")?;
    let protocol = scan_u32(raw, "\"protocol\"")?;
    Some((schema, protocol))
}

fn scan_u32(raw: &str, key: &str) -> Option<u32> {
    let start = raw.find(key)? + key.len();
    let rest = raw.get(start..)?;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A runner that replays scripted outputs and records the argv it
    /// was asked for. No process is ever spawned.
    struct ScriptedRunner {
        outputs: RefCell<Vec<Result<CommandOutput, CommandError>>>,
        seen: RefCell<Vec<Vec<String>>>,
    }

    impl ScriptedRunner {
        fn new(outputs: Vec<Result<CommandOutput, CommandError>>) -> Self {
            Self {
                outputs: RefCell::new(outputs),
                seen: RefCell::new(Vec::new()),
            }
        }

        fn ok(stdout: &str) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                status: 0,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(
            &self,
            argv: &[String],
            _deadline: Timestamp,
        ) -> Result<CommandOutput, CommandError> {
            self.seen.borrow_mut().push(argv.to_vec());
            let mut outputs = self.outputs.borrow_mut();
            if outputs.is_empty() {
                return Err(CommandError::Unavailable);
            }
            outputs.remove(0)
        }
    }

    fn pin() -> HerdrPin {
        HerdrPin {
            version: "0.8.0".to_owned(),
            api_schema_version: 1,
            api_protocol: 19,
        }
    }

    fn snapshot(schema: u32, protocol: u32) -> String {
        format!("{{\"schema_version\": {schema}, \"protocol\": {protocol}, \"panes\": []}}")
    }

    fn provider(
        outputs: Vec<Result<CommandOutput, CommandError>>,
    ) -> HerdrProvider<ScriptedRunner> {
        HerdrProvider::new(ScriptedRunner::new(outputs), pin(), "/pinned/herdr")
    }

    #[test]
    fn matching_version_and_protocol_verify() {
        let provider = provider(vec![
            ScriptedRunner::ok("herdr 0.8.0"),
            ScriptedRunner::ok(&snapshot(1, 19)),
        ]);
        assert_eq!(provider.verify_pinned_identity(Timestamp(100)), Ok(()));
    }

    #[test]
    fn the_pinned_executable_is_invoked_not_a_path_lookup() {
        let provider = provider(vec![
            ScriptedRunner::ok("herdr 0.8.0"),
            ScriptedRunner::ok(&snapshot(1, 19)),
        ]);
        provider
            .verify_pinned_identity(Timestamp(100))
            .expect("identity verifies");
        let seen = provider.runner.seen.borrow();
        assert_eq!(
            seen[0][0], "/pinned/herdr",
            "the adapter must run the pinned executable, never resolve `herdr` from PATH"
        );
        assert_eq!(seen[0][1], "--version");
        assert_eq!(seen[1][1..], ["api", "snapshot"]);
    }

    #[test]
    fn a_drifted_version_refuses() {
        let provider = provider(vec![
            ScriptedRunner::ok("herdr 0.7.5"),
            ScriptedRunner::ok(&snapshot(1, 19)),
        ]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::VersionMismatch)
        );
    }

    #[test]
    fn the_protocol_change_that_forced_the_pin_refuses() {
        // 17 is v0.7.5's protocol. This is the exact drift the
        // compatibility record calls a full pin change.
        let provider = provider(vec![
            ScriptedRunner::ok("herdr 0.8.0"),
            ScriptedRunner::ok(&snapshot(1, 17)),
        ]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::VersionMismatch)
        );
    }

    #[test]
    fn a_drifted_schema_version_refuses() {
        let provider = provider(vec![
            ScriptedRunner::ok("herdr 0.8.0"),
            ScriptedRunner::ok(&snapshot(2, 19)),
        ]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::VersionMismatch)
        );
    }

    #[test]
    fn unreadable_snapshot_output_fails_closed() {
        let provider = provider(vec![
            ScriptedRunner::ok("herdr 0.8.0"),
            ScriptedRunner::ok("<html>not json</html>"),
        ]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::VersionMismatch),
            "output we cannot parse is a mismatch, never an assumed match"
        );
    }

    #[test]
    fn a_snapshot_missing_the_protocol_is_not_a_partial_success() {
        let provider = provider(vec![
            ScriptedRunner::ok("herdr 0.8.0"),
            ScriptedRunner::ok("{\"schema_version\": 1}"),
        ]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::VersionMismatch)
        );
    }

    #[test]
    fn a_nonzero_exit_refuses_rather_than_reading_stdout() {
        let provider = provider(vec![Ok(CommandOutput {
            status: 1,
            stdout: "herdr 0.8.0".to_owned(),
            stderr: "boom".to_owned(),
        })]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::VersionMismatch)
        );
    }

    #[test]
    fn host_refusal_stays_not_permitted_rather_than_becoming_a_mismatch() {
        // A sandbox denial is an agent-boundary fact, not evidence
        // about the provider's identity. Collapsing it into
        // VersionMismatch would send an operator to the wrong problem.
        let provider = provider(vec![Err(CommandError::NotPermitted)]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::NotPermitted)
        );
    }

    #[test]
    fn an_unavailable_binary_is_unavailable_not_a_mismatch() {
        let provider = provider(vec![Err(CommandError::Unavailable)]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::Unavailable)
        );
    }

    #[test]
    fn a_timed_out_identity_check_fails_closed() {
        let provider = provider(vec![Err(CommandError::Timeout)]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::VersionMismatch),
            "an unproven identity refuses; a slow provider is not a verified one"
        );
    }

    #[test]
    fn the_snapshot_is_not_requested_once_the_version_already_drifted() {
        let provider = provider(vec![
            ScriptedRunner::ok("herdr 0.7.5"),
            ScriptedRunner::ok(&snapshot(1, 19)),
        ]);
        let _ = provider.verify_pinned_identity(Timestamp(100));
        assert_eq!(
            provider.runner.seen.borrow().len(),
            1,
            "verification stops at the first proven mismatch"
        );
    }

    #[test]
    fn a_v_prefixed_version_line_parses() {
        let provider = provider(vec![
            ScriptedRunner::ok("herdr v0.8.0"),
            ScriptedRunner::ok(&snapshot(1, 19)),
        ]);
        assert_eq!(provider.verify_pinned_identity(Timestamp(100)), Ok(()));
    }

    #[test]
    fn an_empty_version_line_refuses() {
        let provider = provider(vec![ScriptedRunner::ok("")]);
        assert_eq!(
            provider.verify_pinned_identity(Timestamp(100)),
            Err(RawRunError::VersionMismatch)
        );
    }
}
