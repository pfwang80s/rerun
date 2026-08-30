//! Production-disarmed open-source ownership, terminal registry, and one-shot capability issuance.
//!
//! This module only models the sealed ownership surfaces required by future strict Web open
//! handoffs.
//! It does not hook into the native viewer or the existing compatibility `open()` / `close()`
//! paths.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::remote_validator::{RemoteObjectValidator, RepresentationConsistency};

static NEXT_OPEN_SOURCE_TOKEN: AtomicU64 = AtomicU64::new(1);

/// One fresh source identity for a single open attempt.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpenSourceToken(u64);

impl OpenSourceToken {
    pub fn fresh_v1() -> Self {
        let mut current = NEXT_OPEN_SOURCE_TOKEN.load(Ordering::Relaxed);
        loop {
            let Some(token) = Self::new_v1(current) else {
                panic!("OpenSourceToken allocator exhausted");
            };
            let next = current.checked_add(1).unwrap_or(0);
            match NEXT_OPEN_SOURCE_TOKEN.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return token,
                Err(observed) => current = observed,
            }
        }
    }

    pub const fn new_v1(token: u64) -> Option<Self> {
        if token == 0 { None } else { Some(Self(token)) }
    }

    #[cfg(test)]
    pub const fn new_for_test_v1(token: u64) -> Self {
        Self(token)
    }

    pub const fn get_v1(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for OpenSourceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpenSourceToken(<opaque>)")
    }
}

/// The first terminal cause wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenSourceTerminalCauseV1 {
    OpeningFailure,
    SessionFatal,
    PresentationCommitPoisoned,
    ExplicitClose,
    MemoryPressure,
    BackgroundPolicy,
    ViewerStopped,
    AdministrativeCleanup,
}

/// Whether a source is still live or already terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenSourceStatusV1 {
    Live,
    Terminal,
}

/// A bounded diagnostic line retained in terminal snapshots.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenSourceDiagnosticV1(String);

impl OpenSourceDiagnosticV1 {
    pub fn new_v1(message: impl AsRef<str>, max_bytes: usize) -> Self {
        let mut message = message.as_ref().to_owned();
        if message.len() > max_bytes {
            let mut truncate_at = max_bytes;
            while !message.is_char_boundary(truncate_at) {
                truncate_at -= 1;
            }
            message.truncate(truncate_at);
        }
        Self(message)
    }

    pub fn wire_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OpenSourceDiagnosticV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpenSourceDiagnosticV1(<redacted>)")
    }
}

/// Frozen terminal state for one source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSourceTerminalSnapshotV1 {
    source_token: OpenSourceToken,
    terminal_revision: u64,
    cause: OpenSourceTerminalCauseV1,
    diagnostics: Vec<OpenSourceDiagnosticV1>,
}

impl OpenSourceTerminalSnapshotV1 {
    pub fn new_v1<I, S>(
        source_token: OpenSourceToken,
        terminal_revision: u64,
        cause: OpenSourceTerminalCauseV1,
        diagnostics_iter: I,
        max_diagnostics: usize,
        max_snapshot_bytes: usize,
        max_diagnostic_bytes: usize,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut remaining_bytes = max_snapshot_bytes.saturating_sub(17);
        let mut diagnostics = Vec::new();
        for message in diagnostics_iter.into_iter().take(max_diagnostics) {
            if remaining_bytes == 0 {
                break;
            }
            let diagnostic =
                OpenSourceDiagnosticV1::new_v1(message, max_diagnostic_bytes.min(remaining_bytes));
            let diagnostic_bytes = diagnostic.wire_str().len();
            if diagnostic_bytes == 0 {
                break;
            }
            remaining_bytes = remaining_bytes.saturating_sub(diagnostic_bytes);
            diagnostics.push(diagnostic);
        }
        Self {
            source_token,
            terminal_revision,
            cause,
            diagnostics,
        }
    }

    pub const fn source_token(&self) -> OpenSourceToken {
        self.source_token
    }

    pub const fn terminal_revision(&self) -> u64 {
        self.terminal_revision
    }

    pub const fn cause(&self) -> OpenSourceTerminalCauseV1 {
        self.cause
    }

    pub fn diagnostics(&self) -> &[OpenSourceDiagnosticV1] {
        &self.diagnostics
    }

    pub fn retained_bytes_v1(&self) -> u64 {
        let diagnostics_bytes = self
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.wire_str().len() as u64)
            .sum::<u64>();
        8 + 8 + 1 + diagnostics_bytes
    }
}

/// One live owner for a source's terminal status.
pub struct OpenSourceStatusOwnerV1 {
    source_token: OpenSourceToken,
    terminal_revision: u64,
    first_terminal_cause: Option<OpenSourceTerminalCauseV1>,
    status: OpenSourceStatusV1,
}

impl OpenSourceStatusOwnerV1 {
    pub fn new_fresh_v1() -> Self {
        Self::new_with_token_v1(OpenSourceToken::fresh_v1())
    }

    pub fn new_with_token_v1(source_token: OpenSourceToken) -> Self {
        Self {
            source_token,
            terminal_revision: 0,
            first_terminal_cause: None,
            status: OpenSourceStatusV1::Live,
        }
    }

    pub const fn source_token(&self) -> OpenSourceToken {
        self.source_token
    }

    pub const fn status(&self) -> OpenSourceStatusV1 {
        self.status
    }

    pub const fn terminal_revision(&self) -> u64 {
        self.terminal_revision
    }

    pub const fn first_terminal_cause(&self) -> Option<OpenSourceTerminalCauseV1> {
        self.first_terminal_cause
    }

    pub fn latch_terminal_v1(&mut self, cause: OpenSourceTerminalCauseV1) -> bool {
        if self.first_terminal_cause.is_some() {
            return false;
        }
        self.first_terminal_cause = Some(cause);
        self.status = OpenSourceStatusV1::Terminal;
        self.terminal_revision = self
            .terminal_revision
            .checked_add(1)
            .expect("open-source terminal revision overflowed");
        true
    }

    pub fn snapshot_v1<I, S>(
        &self,
        diagnostics_iter: I,
        max_diagnostics: usize,
        max_snapshot_bytes: usize,
        max_diagnostic_bytes: usize,
    ) -> Option<OpenSourceTerminalSnapshotV1>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Some(OpenSourceTerminalSnapshotV1::new_v1(
            self.source_token,
            self.terminal_revision,
            self.first_terminal_cause?,
            diagnostics_iter,
            max_diagnostics,
            max_snapshot_bytes,
            max_diagnostic_bytes,
        ))
    }
}

impl fmt::Debug for OpenSourceStatusOwnerV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenSourceStatusOwnerV1")
            .field("source_token", &"<opaque>")
            .field("terminal_revision", &self.terminal_revision)
            .field("status", &self.status)
            .field("first_terminal_cause", &self.first_terminal_cause)
            .finish()
    }
}

/// The complete object observed after the probe produces one typed remote object.
#[derive(Clone, Debug)]
pub struct BoundRemoteObjectV1 {
    pub identity: RemoteObjectIdentityV1,
    pub validator: RemoteObjectValidator,
}

impl BoundRemoteObjectV1 {
    pub fn new_v1(identity: RemoteObjectIdentityV1, validator: RemoteObjectValidator) -> Self {
        Self {
            identity,
            validator,
        }
    }
}

/// One typed remote-object identity frozen after the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteObjectIdentityV1 {
    pub source_token: OpenSourceToken,
    pub content_length: u64,
    pub consistency: RepresentationConsistency,
}

impl RemoteObjectIdentityV1 {
    pub const fn new_v1(
        source_token: OpenSourceToken,
        content_length: u64,
        consistency: RepresentationConsistency,
    ) -> Self {
        Self {
            source_token,
            content_length,
            consistency,
        }
    }
}

/// One-shot issuer authority for a matching remote opening token.
pub struct BoundRemoteObjectCapabilityIssuerV1 {
    source_token: OpenSourceToken,
    spent: bool,
}

impl BoundRemoteObjectCapabilityIssuerV1 {
    pub const fn new_v1(source_token: OpenSourceToken) -> Self {
        Self {
            source_token,
            spent: false,
        }
    }

    pub fn issue_v1(
        &mut self,
        object: BoundRemoteObjectV1,
    ) -> Result<BoundRemoteObjectCapabilityV1, BoundRemoteObjectCapabilityIssuerErrorV1> {
        if self.spent {
            return Err(BoundRemoteObjectCapabilityIssuerErrorV1::AlreadySpent);
        }
        self.spent = true;
        if object.identity.source_token != self.source_token {
            return Err(BoundRemoteObjectCapabilityIssuerErrorV1::WrongSourceToken);
        }
        Ok(BoundRemoteObjectCapabilityV1 { object })
    }
}

impl fmt::Debug for BoundRemoteObjectCapabilityIssuerV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BoundRemoteObjectCapabilityIssuerV1(<opaque>)")
    }
}

/// Failure to issue the one-shot capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundRemoteObjectCapabilityIssuerErrorV1 {
    AlreadySpent,
    WrongSourceToken,
}

/// A sealed capability that cannot be reconstructed from scalars.
pub struct BoundRemoteObjectCapabilityV1 {
    object: BoundRemoteObjectV1,
}

impl BoundRemoteObjectCapabilityV1 {
    pub fn object(&self) -> &BoundRemoteObjectV1 {
        &self.object
    }
}

impl fmt::Debug for BoundRemoteObjectCapabilityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BoundRemoteObjectCapabilityV1(<opaque>)")
    }
}

/// Bounded registry of terminal snapshots.
pub struct OpenSourceTerminalRegistryV1 {
    max_entries: usize,
    max_bytes: u64,
    _max_diagnostics: usize,
    _max_diagnostic_bytes: usize,
    entries: VecDeque<OpenSourceTerminalSnapshotV1>,
    retained_bytes: u64,
}

impl OpenSourceTerminalRegistryV1 {
    pub fn new_v1(
        max_entries: usize,
        max_bytes: u64,
        max_diagnostics: usize,
        max_diagnostic_bytes: usize,
    ) -> Self {
        Self {
            max_entries,
            max_bytes,
            _max_diagnostics: max_diagnostics,
            _max_diagnostic_bytes: max_diagnostic_bytes,
            entries: VecDeque::new(),
            retained_bytes: 0,
        }
    }

    pub fn retained_bytes_v1(&self) -> u64 {
        self.retained_bytes
    }

    pub fn entries_v1(&self) -> impl ExactSizeIterator<Item = &OpenSourceTerminalSnapshotV1> {
        self.entries.iter()
    }

    pub fn record_v1(
        &mut self,
        snapshot: OpenSourceTerminalSnapshotV1,
    ) -> OpenSourceTerminalRegistryOutcomeV1 {
        if self
            .entries
            .iter()
            .any(|entry| entry.source_token == snapshot.source_token)
        {
            return OpenSourceTerminalRegistryOutcomeV1::Reused;
        }

        let snapshot_bytes = snapshot.retained_bytes_v1();
        if snapshot_bytes > self.max_bytes {
            return OpenSourceTerminalRegistryOutcomeV1::RejectedTooLarge;
        }
        self.entries.push_back(snapshot);
        self.retained_bytes = self
            .retained_bytes
            .checked_add(snapshot_bytes)
            .expect("open-source terminal registry bytes overflowed");

        let mut evicted = Vec::new();
        while self.entries.len() > self.max_entries || self.retained_bytes > self.max_bytes {
            let Some(removed) = self.entries.pop_front() else {
                break;
            };
            self.retained_bytes = self
                .retained_bytes
                .checked_sub(removed.retained_bytes_v1())
                .expect("open-source terminal registry bytes underflowed");
            evicted.push(removed.source_token());
        }

        OpenSourceTerminalRegistryOutcomeV1::Inserted {
            evicted_tokens: evicted,
        }
    }

    pub fn snapshot_for_v1(
        &self,
        source_token: OpenSourceToken,
    ) -> Option<&OpenSourceTerminalSnapshotV1> {
        self.entries
            .iter()
            .find(|entry| entry.source_token == source_token)
    }
}

/// Registry insertion result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenSourceTerminalRegistryOutcomeV1 {
    Inserted {
        evicted_tokens: Vec<OpenSourceToken>,
    },
    Reused,
    RejectedTooLarge,
}

impl fmt::Debug for OpenSourceTerminalRegistryV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenSourceTerminalRegistryV1")
            .field("max_entries", &self.max_entries)
            .field("max_bytes", &self.max_bytes)
            .field("_max_diagnostics", &self._max_diagnostics)
            .field("_max_diagnostic_bytes", &self._max_diagnostic_bytes)
            .field("entries", &self.entries.len())
            .field("retained_bytes", &self.retained_bytes)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(v: u64) -> OpenSourceToken {
        OpenSourceToken::new_for_test_v1(v)
    }

    #[test]
    fn first_terminal_cause_wins_and_revision_latches_once() {
        let mut owner = OpenSourceStatusOwnerV1::new_with_token_v1(token(7));
        assert_eq!(owner.status(), OpenSourceStatusV1::Live);
        assert_eq!(owner.terminal_revision(), 0);
        assert!(owner.latch_terminal_v1(OpenSourceTerminalCauseV1::PresentationCommitPoisoned));
        assert_eq!(owner.status(), OpenSourceStatusV1::Terminal);
        assert_eq!(owner.terminal_revision(), 1);
        assert_eq!(
            owner.first_terminal_cause(),
            Some(OpenSourceTerminalCauseV1::PresentationCommitPoisoned)
        );
        assert!(!owner.latch_terminal_v1(OpenSourceTerminalCauseV1::ExplicitClose));
        assert_eq!(
            owner.first_terminal_cause(),
            Some(OpenSourceTerminalCauseV1::PresentationCommitPoisoned)
        );
    }

    #[test]
    fn terminal_registry_deduplicates_by_source_and_evicts_oldest() {
        let mut registry = OpenSourceTerminalRegistryV1::new_v1(2, 80, 4, 16);
        let first = OpenSourceStatusOwnerV1::new_with_token_v1(token(1));
        let second = OpenSourceStatusOwnerV1::new_with_token_v1(token(2));
        let third = OpenSourceStatusOwnerV1::new_with_token_v1(token(3));
        let snapshot = |owner: &OpenSourceStatusOwnerV1, cause, note: &str| {
            let mut owner = OpenSourceStatusOwnerV1 {
                source_token: owner.source_token(),
                terminal_revision: owner.terminal_revision(),
                first_terminal_cause: owner.first_terminal_cause(),
                status: owner.status(),
            };
            owner.latch_terminal_v1(cause);
            owner
                .snapshot_v1([note], 4, 80, 16)
                .expect("terminal snapshot should exist")
        };

        let first_snapshot = snapshot(&first, OpenSourceTerminalCauseV1::ExplicitClose, "first");
        let second_snapshot =
            snapshot(&second, OpenSourceTerminalCauseV1::MemoryPressure, "second");
        let third_snapshot = snapshot(&third, OpenSourceTerminalCauseV1::BackgroundPolicy, "third");

        assert!(matches!(
            registry.record_v1(first_snapshot.clone()),
            OpenSourceTerminalRegistryOutcomeV1::Inserted { evicted_tokens } if evicted_tokens.is_empty()
        ));
        assert!(matches!(
            registry.record_v1(second_snapshot.clone()),
            OpenSourceTerminalRegistryOutcomeV1::Inserted { evicted_tokens } if evicted_tokens.is_empty()
        ));
        assert_eq!(registry.entries_v1().len(), 2);
        assert!(matches!(
            registry.record_v1(first_snapshot),
            OpenSourceTerminalRegistryOutcomeV1::Reused
        ));

        assert!(matches!(
            registry.record_v1(third_snapshot),
            OpenSourceTerminalRegistryOutcomeV1::Inserted { evicted_tokens } if evicted_tokens == vec![token(1)]
        ));
        assert_eq!(registry.entries_v1().len(), 2);
        assert!(registry.snapshot_for_v1(token(1)).is_none());
        assert!(registry.snapshot_for_v1(token(2)).is_some());
        assert!(registry.snapshot_for_v1(token(3)).is_some());
    }

    #[test]
    fn capability_issuer_is_one_shot_and_token_checked() {
        let source_token = token(11);
        let identity = RemoteObjectIdentityV1::new_v1(
            source_token,
            123,
            RepresentationConsistency::StrongValidator,
        );
        let object = BoundRemoteObjectV1::new_v1(
            identity,
            RemoteObjectValidator::DeploymentAssumed {
                observed_weak: None,
            },
        );
        let mut issuer = BoundRemoteObjectCapabilityIssuerV1::new_v1(source_token);
        let capability = issuer
            .issue_v1(object)
            .expect("matching token should issue");
        assert_eq!(capability.object().identity.source_token, source_token);
        assert!(matches!(
            issuer.issue_v1(BoundRemoteObjectV1::new_v1(
                identity,
                RemoteObjectValidator::DeploymentAssumed {
                    observed_weak: None
                },
            )),
            Err(BoundRemoteObjectCapabilityIssuerErrorV1::AlreadySpent)
        ));

        let mut wrong_issuer = BoundRemoteObjectCapabilityIssuerV1::new_v1(token(12));
        assert!(matches!(
            wrong_issuer.issue_v1(BoundRemoteObjectV1::new_v1(
                identity,
                RemoteObjectValidator::DeploymentAssumed {
                    observed_weak: None
                },
            )),
            Err(BoundRemoteObjectCapabilityIssuerErrorV1::WrongSourceToken)
        ));
    }

    #[test]
    fn diagnostic_truncation_respects_utf8_boundaries() {
        let diagnostic = OpenSourceDiagnosticV1::new_v1("éé", 1);
        assert_eq!(diagnostic.wire_str(), "");
        let diagnostic = OpenSourceDiagnosticV1::new_v1("éé", 2);
        assert_eq!(diagnostic.wire_str(), "é");
    }
}
