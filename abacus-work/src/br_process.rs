//! Process seam for the pinned `br` provider (`omw.2`).
//!
//! The adapter never synthesizes shell strings: every invocation is an
//! argv vector handed to a [`BrRunner`], and the runner owns the fixed
//! binary path, control-checkout working directory, sanitized
//! environment, deadline, and output bound. Hermetic tests script the
//! runner from `abacus-work/fixtures/br-v0.1.45` observations; only the
//! live lane (pin changes, explicit manual invocation) runs a real
//! process.

use std::cell::RefCell;

use abacus_core::ports::WorkError;

/// One argv invocation of the pinned binary. Carries only what varies
/// per call; binary identity, cwd, env, deadline, and output bound are
/// runner configuration, so a request cannot smuggle any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrRequest {
    pub args: Vec<String>,
}

impl BrRequest {
    pub fn new<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// What one completed process reported. Exit code and both streams are
/// facts; interpreting them is the adapter's job, never the runner's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrObservation {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Transport-level failure: the process could not run to a reportable
/// completion. A mutation caller must treat `DeadlineExceeded` and
/// `OutputBoundExceeded` as AMBIGUOUS (the write may be in flight); the
/// distinction from a clean nonzero exit is load-bearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrRunError {
    /// The process never started; nothing can have taken effect.
    Spawn,
    /// The deadline elapsed with the process still running.
    DeadlineExceeded,
    /// Output exceeded the configured bound and the process was stopped.
    OutputBoundExceeded,
}

/// Runs one pinned-binary invocation.
pub trait BrRunner {
    fn run(&self, request: &BrRequest) -> Result<BrObservation, BrRunError>;
}

/// Scripted runner for hermetic tests: expected requests paired with
/// canned results, consumed in order. An unexpected or leftover request
/// is a test defect and panics loudly.
#[derive(Debug, Default)]
pub struct ScriptedBrRunner {
    steps: RefCell<Vec<(BrRequest, Result<BrObservation, BrRunError>)>>,
}

impl ScriptedBrRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn expect(self, request: BrRequest, result: Result<BrObservation, BrRunError>) -> Self {
        self.steps.borrow_mut().push((request, result));
        self
    }

    /// Panics if scripted steps were never consumed.
    pub fn assert_exhausted(&self) {
        assert!(
            self.steps.borrow().is_empty(),
            "scripted br steps left unconsumed"
        );
    }
}

impl BrRunner for ScriptedBrRunner {
    fn run(&self, request: &BrRequest) -> Result<BrObservation, BrRunError> {
        let mut steps = self.steps.borrow_mut();
        assert!(
            !steps.is_empty(),
            "unexpected br invocation: {:?}",
            request.args
        );
        let (expected, result) = steps.remove(0);
        assert_eq!(expected, *request, "br invocation out of scripted order");
        result
    }
}

/// Fail-closed pin gate: one `--version` read must report exactly the
/// pinned identity line before any mutation is issued. A mismatch is
/// `Incompatible`, never a fallback; a transport failure on this
/// read-only probe is `ProviderUnavailable` (nothing was mutated).
pub fn verify_pinned_identity<R: BrRunner>(
    runner: &R,
    pinned_version_line: &str,
) -> Result<(), WorkError> {
    let observation = runner
        .run(&BrRequest::new(["--version"]))
        .map_err(|_| WorkError::ProviderUnavailable)?;
    if observation.exit_code == 0 && observation.stdout.trim() == pinned_version_line {
        Ok(())
    } else {
        Err(WorkError::Incompatible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: &str = "br 0.1.45";

    fn version_request() -> BrRequest {
        BrRequest::new(["--version"])
    }

    fn observation(exit_code: i32, stdout: &str) -> BrObservation {
        BrObservation {
            exit_code,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    #[test]
    fn pinned_identity_match_passes() {
        let runner =
            ScriptedBrRunner::new().expect(version_request(), Ok(observation(0, "br 0.1.45\n")));
        assert_eq!(verify_pinned_identity(&runner, PINNED), Ok(()));
        runner.assert_exhausted();
    }

    #[test]
    fn version_drift_fails_closed_as_incompatible() {
        let runner =
            ScriptedBrRunner::new().expect(version_request(), Ok(observation(0, "br 0.1.99\n")));
        assert_eq!(
            verify_pinned_identity(&runner, PINNED),
            Err(WorkError::Incompatible)
        );
    }

    #[test]
    fn nonzero_version_exit_fails_closed_as_incompatible() {
        let runner = ScriptedBrRunner::new().expect(version_request(), Ok(observation(2, "")));
        assert_eq!(
            verify_pinned_identity(&runner, PINNED),
            Err(WorkError::Incompatible)
        );
    }

    #[test]
    fn transport_failure_on_the_probe_is_provider_unavailable() {
        for error in [
            BrRunError::Spawn,
            BrRunError::DeadlineExceeded,
            BrRunError::OutputBoundExceeded,
        ] {
            let runner = ScriptedBrRunner::new().expect(version_request(), Err(error));
            assert_eq!(
                verify_pinned_identity(&runner, PINNED),
                Err(WorkError::ProviderUnavailable),
                "probe transport failure {error:?} must read as unavailable"
            );
        }
    }
}
