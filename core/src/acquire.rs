//! Pickup-gate collapse — the engine-generic fold for a return-value inbound hook.
//!
//! Spec `docs/superpowers/specs/2026-08-14-pickup-gates-design.md` §5.
//! `Allowed` is 0. Any other i32 is a deny. Two denys do not have a severity order: the caller
//! supplies votes in HookResult-precedence then registration order, and the first deny sticks.

/// Engine `AcquireResult::Allowed`. Every other documented engine code is a deny.
pub const ACQUIRE_ALLOWED: i32 = 0;
/// Implicit deny when a handler returns Handled/Stop without writing `result`.
pub const ACQUIRE_IMPLICIT_DENY: i32 = 1; // InvalidItem

/// Most-restrictive of two acquire codes: any non-Allow beats Allow; if both deny, `first` wins
/// (the caller already sorted by HookResult precedence then registration order).
pub fn most_restrictive(first: i32, second: i32) -> i32 {
    if first != ACQUIRE_ALLOWED {
        first
    } else {
        second
    }
}

/// One Pre handler's vote. `Continue` is not a vote and must not be pushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcquireVote {
    pub result: i32,
    /// True when the handler returned Handled/Stop (skip original).
    pub skip_original: bool,
}

/// Put Handled/Stop votes ahead of Changed, keeping registration order within each group.
/// `fold_acquire` then first-wins among denys, which is HookResult-precedence then registration.
pub fn order_votes(votes: &mut [AcquireVote]) {
    votes.sort_by(|a, b| b.skip_original.cmp(&a.skip_original));
}

/// Fold Pre votes then, if the original ran, the engine return.
///
/// `votes` must already be in HookResult-precedence then registration order so two denys pick
/// the first. An empty vote list with no engine return yields Allowed (nobody spoke).
pub fn fold_acquire(votes: &[AcquireVote], engine: Option<i32>) -> (i32, bool) {
    let mut folded = ACQUIRE_ALLOWED;
    let mut voted = false;
    let mut skip = false;
    for v in votes {
        if !voted {
            folded = v.result;
            voted = true;
        } else {
            folded = most_restrictive(folded, v.result);
        }
        if v.skip_original {
            skip = true;
        }
    }
    if skip {
        if !voted {
            folded = ACQUIRE_IMPLICIT_DENY;
        }
        return (folded, true);
    }
    if let Some(eng) = engine {
        folded = if voted { most_restrictive(folded, eng) } else { eng };
    }
    (folded, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_loses_to_any_deny() {
        assert_eq!(most_restrictive(ACQUIRE_ALLOWED, 6), 6);
        assert_eq!(most_restrictive(6, ACQUIRE_ALLOWED), 6);
        assert_eq!(most_restrictive(2, 6), 2, "two denys: first wins, no ontology");
    }

    #[test]
    fn continue_only_takes_the_engine() {
        let (r, skip) = fold_acquire(&[], Some(0));
        assert_eq!((r, skip), (0, false));
        let (r, skip) = fold_acquire(&[], Some(3));
        assert_eq!((r, skip), (3, false));
    }

    #[test]
    fn handled_skips_and_implicit_deny_when_unset() {
        let votes = [AcquireVote { result: ACQUIRE_IMPLICIT_DENY, skip_original: true }];
        let (r, skip) = fold_acquire(&votes, Some(0));
        assert_eq!((r, skip), (ACQUIRE_IMPLICIT_DENY, true));
    }

    #[test]
    fn changed_deny_then_engine_allow_still_denies() {
        let votes = [AcquireVote { result: 2, skip_original: false }];
        let (r, skip) = fold_acquire(&votes, Some(ACQUIRE_ALLOWED));
        assert_eq!((r, skip), (2, false));
    }

    #[test]
    fn changed_allow_then_engine_deny_denies() {
        let votes = [AcquireVote { result: ACQUIRE_ALLOWED, skip_original: false }];
        let (r, skip) = fold_acquire(&votes, Some(6));
        assert_eq!((r, skip), (6, false));
    }

    #[test]
    fn handled_outranks_changed_when_both_deny() {
        let mut votes = [
            AcquireVote { result: 6, skip_original: false },
            AcquireVote { result: 2, skip_original: true },
        ];
        order_votes(&mut votes);
        let (r, skip) = fold_acquire(&votes, None);
        assert_eq!((r, skip), (2, true), "Handled deny beats an earlier Changed deny");
    }

    #[test]
    fn first_deny_wins_between_two_handled() {
        let votes = [
            AcquireVote { result: 2, skip_original: true },
            AcquireVote { result: 6, skip_original: true },
        ];
        let (r, skip) = fold_acquire(&votes, None);
        assert_eq!((r, skip), (2, true));
    }
}
