//! How a turn settles.
//!
//! Two closed vocabularies from the contract, and the one rule that reads the
//! first as the second:
//!
//! - [`SessionStatus`] — `OrchestrationSessionStatus`, seven literals
//!   (`packages/contracts/src/orchestration.ts:261`). What the agent process is
//!   doing.
//! - [`TurnState`] — `OrchestrationLatestTurn["state"]`, four. How the most
//!   recent turn went.
//!
//! [`SessionStatus::settles_turn_as`] is the rule. **Leaving `running` is the
//! end of a turn** — not the last assistant message — which is what makes a
//! turn's duration cover the whole turn.
//!
//! # Why this is a module and not two `match`es
//!
//! Upstream writes the rule down twice, character for character: once in its
//! server (`apps/server/src/orchestration/Layers/ProjectionPipeline.ts:78`) and
//! once in its client (`packages/client-runtime/src/state/threadReducer.ts:539`).
//! It has to, because both fold the same events into the same read model — a
//! client that watched every event and one that arrives late and takes a
//! snapshot must see the same conversation.
//!
//! laplus reuses that client unmodified, so this is the **third** copy and
//! the only one under this repository's control. Its correctness is not a
//! matter of opinion: it is whether it agrees with the other two. That is what
//! the tests below assert, and why they name their source.
//!
//! The inverse — *what status to send* — is deliberately **not** here.
//! Upstream keeps it per-provider ([`ProviderRuntimeIngestion`'s
//! `orchestrationSessionStatusFromRuntimeState`], and a second one in
//! `ProviderCommandReactor`), and laplus's equivalent is
//! `crate::turn::Ending::session_status`, which is the one thing on this path
//! that knows about the `claude` CLI. A second driver would bring its own; it
//! would not bring another copy of this.

/// What the agent process behind a thread is doing.
///
/// The contract's `OrchestrationSessionStatus`, in its declared order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// A thread with no turn in progress and nothing wrong.
    Idle,
    /// The process is being started. Says nothing about how a turn went.
    Starting,
    /// A turn is in progress.
    Running,
    /// Started, idle, and ready for the next turn.
    Ready,
    /// The developer stopped the turn.
    Interrupted,
    /// The process is gone. Distinct from [`SessionStatus::Interrupted`] at the
    /// session level — nobody asked — but a turn caught by it settles the same
    /// way, because from the turn's point of view it did not finish.
    Stopped,
    /// Something went wrong; `lastError` says what.
    Error,
}

/// How the most recent turn went.
///
/// The contract's `OrchestrationLatestTurn["state"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Running,
    Completed,
    Interrupted,
    Error,
}

impl SessionStatus {
    /// The state to settle a still-running turn with, or `None` while the
    /// session is starting or running and the turn must stay unsettled.
    ///
    /// Mirrors `settledTurnStateForSessionStatus`. Both upstream copies are
    /// reproduced in `the_rule_matches_upstreams_two_copies` below; change this
    /// only by re-reading them.
    pub fn settles_turn_as(self) -> Option<TurnState> {
        match self {
            SessionStatus::Idle | SessionStatus::Ready => Some(TurnState::Completed),
            SessionStatus::Error => Some(TurnState::Error),
            SessionStatus::Interrupted | SessionStatus::Stopped => Some(TurnState::Interrupted),
            SessionStatus::Starting | SessionStatus::Running => None,
        }
    }

    /// Is there an agent working behind this conversation right now?
    ///
    /// `starting` and `running`, which are exactly the two
    /// [`SessionStatus::settles_turn_as`] has no answer for — a status that says
    /// nothing about how a turn went says so because the turn is not over.
    ///
    /// Named rather than matched in place because it is read from two directions
    /// that must not be allowed to disagree:
    /// [`crate::threads::Thread::busy`] refuses a settle with it, and
    /// [`crate::threads::Change::wakes_the_inbox`] resets an override with it.
    /// A conversation the developer *cannot settle* because an agent is working
    /// is the same conversation whose settle a starting agent *undoes*, and a
    /// second reading of the enum would let those two part company — which would
    /// show as a status arriving after the fact quietly undoing a decision it was
    /// never meant to touch.
    pub fn is_working(self) -> bool {
        matches!(self, SessionStatus::Starting | SessionStatus::Running)
    }

    /// The literal the contract puts on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Idle => "idle",
            SessionStatus::Starting => "starting",
            SessionStatus::Running => "running",
            SessionStatus::Ready => "ready",
            SessionStatus::Interrupted => "interrupted",
            SessionStatus::Stopped => "stopped",
            SessionStatus::Error => "error",
        }
    }
}

impl TurnState {
    /// A stored turn state, back as one of the contract's four.
    ///
    /// An unrecognised state becomes [`TurnState::Error`] rather than
    /// `Completed`, because a turn whose outcome cannot be read is not one to
    /// report as having gone well. The same reasoning as
    /// `crate::threads::tone`.
    pub fn from_stored(stored: &str) -> TurnState {
        match stored {
            "running" => TurnState::Running,
            "interrupted" => TurnState::Interrupted,
            "completed" => TurnState::Completed,
            _ => TurnState::Error,
        }
    }

    /// The literal the contract puts on the wire, and the one
    /// [`crate::store`] keeps.
    pub fn as_str(self) -> &'static str {
        match self {
            TurnState::Running => "running",
            TurnState::Completed => "completed",
            TurnState::Interrupted => "interrupted",
            TurnState::Error => "error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both upstream copies, transcribed. The assertion is agreement, so the
    /// table is written out here rather than derived from the implementation —
    /// a table generated from the thing it checks would pass no matter what the
    /// thing said.
    ///
    /// Sources, both at upstream `5719e8a`:
    /// - `apps/server/src/orchestration/Layers/ProjectionPipeline.ts:78`
    /// - `packages/client-runtime/src/state/threadReducer.ts:539`
    #[test]
    fn the_rule_matches_upstreams_two_copies() {
        let upstream: &[(SessionStatus, Option<TurnState>)] = &[
            (SessionStatus::Idle, Some(TurnState::Completed)),
            (SessionStatus::Ready, Some(TurnState::Completed)),
            (SessionStatus::Error, Some(TurnState::Error)),
            (SessionStatus::Interrupted, Some(TurnState::Interrupted)),
            (SessionStatus::Stopped, Some(TurnState::Interrupted)),
            (SessionStatus::Starting, None),
            (SessionStatus::Running, None),
        ];

        for (status, settled) in upstream {
            assert_eq!(
                status.settles_turn_as(),
                *settled,
                "{} settles a running turn differently from upstream",
                status.as_str()
            );
        }
    }

    /// The seven the contract declares, and no eighth. `settles_turn_as` is a
    /// total function over them, so a status added upstream is a compile error
    /// here rather than a silent fall-through.
    #[test]
    fn every_contract_status_is_covered() {
        let all = [
            SessionStatus::Idle,
            SessionStatus::Starting,
            SessionStatus::Running,
            SessionStatus::Ready,
            SessionStatus::Interrupted,
            SessionStatus::Stopped,
            SessionStatus::Error,
        ];
        assert_eq!(all.len(), 7);
        for status in all {
            // Total, and the literal is one the contract declares.
            let _ = status.settles_turn_as();
            assert!(
                [
                    "idle",
                    "starting",
                    "running",
                    "ready",
                    "interrupted",
                    "stopped",
                    "error"
                ]
                .contains(&status.as_str()),
                "{} is not a contract literal",
                status.as_str()
            );
        }
    }

    /// A settled turn is never settled as `running` — the whole point of
    /// settling is that the turn stopped.
    #[test]
    fn settling_never_produces_a_running_turn() {
        for status in [
            SessionStatus::Idle,
            SessionStatus::Starting,
            SessionStatus::Running,
            SessionStatus::Ready,
            SessionStatus::Interrupted,
            SessionStatus::Stopped,
            SessionStatus::Error,
        ] {
            assert_ne!(status.settles_turn_as(), Some(TurnState::Running));
        }
    }

    /// An agent is working exactly while the status has nothing to say about how
    /// a turn went, which is the agreement [`SessionStatus::is_working`] claims
    /// and the two readings of it depend on: the settle it refuses and the
    /// override a starting agent resets are the same set of conversations.
    #[test]
    fn an_agent_is_working_exactly_while_no_turn_has_settled() {
        for status in [
            SessionStatus::Idle,
            SessionStatus::Starting,
            SessionStatus::Running,
            SessionStatus::Ready,
            SessionStatus::Interrupted,
            SessionStatus::Stopped,
            SessionStatus::Error,
        ] {
            assert_eq!(
                status.is_working(),
                status.settles_turn_as().is_none(),
                "{} is read as working by one rule and not the other",
                status.as_str()
            );
        }
    }

    #[test]
    fn a_stored_turn_state_round_trips() {
        for state in [
            TurnState::Running,
            TurnState::Completed,
            TurnState::Interrupted,
            TurnState::Error,
        ] {
            assert_eq!(TurnState::from_stored(state.as_str()), state);
        }
    }

    #[test]
    fn a_turn_state_this_build_cannot_read_is_an_error_rather_than_a_success() {
        assert_eq!(TurnState::from_stored("snoozed"), TurnState::Error);
        assert_eq!(TurnState::from_stored(""), TurnState::Error);
    }
}
