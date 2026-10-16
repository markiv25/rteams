use serde::{Deserialize, Serialize};

/// Task lifecycle: pending -> in_progress -> review -> approved -> done,
/// plus `needs_fix` (review bounced) and `blocked` (parked, remembers where it came from).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    InProgress,
    Review,
    Approved,
    Done,
    NeedsFix,
    Blocked,
}

impl Status {
    pub const ALL: [Status; 7] = [
        Status::Pending,
        Status::InProgress,
        Status::Review,
        Status::Approved,
        Status::Done,
        Status::NeedsFix,
        Status::Blocked,
    ];

    /// The single table of legal edges. Everything that changes `status` goes through it.
    pub fn can_transition_to(self, to: Status) -> bool {
        use Status::*;
        matches!(
            (self, to),
            (Pending, InProgress)
                | (Pending, Blocked)
                | (InProgress, Review)
                | (InProgress, Done) // no-review path
                | (InProgress, Blocked)
                | (Review, Approved)
                | (Review, NeedsFix)
                | (Approved, Done)
                | (NeedsFix, InProgress)
                | (NeedsFix, Blocked)
                | (Done, InProgress) // reopen for follow-up work
                // unblock: only back to the status the task was blocked from
                | (Blocked, Pending)
                | (Blocked, InProgress)
                | (Blocked, NeedsFix)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::*;

    const LEGAL: [(Status, Status); 14] = [
        (Pending, InProgress),
        (Pending, Blocked),
        (InProgress, Review),
        (InProgress, Done),
        (InProgress, Blocked),
        (Review, Approved),
        (Review, NeedsFix),
        (Approved, Done),
        (NeedsFix, InProgress),
        (NeedsFix, Blocked),
        (Done, InProgress),
        (Blocked, Pending),
        (Blocked, InProgress),
        (Blocked, NeedsFix),
    ];

    /// Golden: the full 7x7 matrix. Anything not in LEGAL must be rejected.
    #[test]
    fn full_transition_matrix() {
        for from in Status::ALL {
            for to in Status::ALL {
                let expected = LEGAL.contains(&(from, to));
                assert_eq!(from.can_transition_to(to), expected, "{from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn representative_illegal() {
        assert!(!Pending.can_transition_to(Done)); // can't skip work
        assert!(!Pending.can_transition_to(Review));
        assert!(!InProgress.can_transition_to(Approved)); // approval only from review
        assert!(!Review.can_transition_to(Done)); // must be approved first
        assert!(!Approved.can_transition_to(NeedsFix));
        assert!(!Done.can_transition_to(Pending));
        assert!(!Blocked.can_transition_to(Done));
        assert!(!InProgress.can_transition_to(InProgress));
    }

    #[test]
    fn serde_names_are_snake_case() {
        assert_eq!(serde_json_name(NeedsFix), "\"needs_fix\"");
        assert_eq!(serde_json_name(InProgress), "\"in_progress\"");
    }

    fn serde_json_name(s: Status) -> String {
        // serde_json is a dev-dependency
        serde_json::to_string(&s).unwrap()
    }
}
