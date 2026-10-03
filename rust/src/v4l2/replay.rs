//! Reference-chain validation before destructive decoder transitions.
//!
//! Parameter sets describe pictures; they do not recreate decoded references.
//! The bounded history must retain a random-access picture and every following
//! dependency. Recovery fails closed when that evidence is missing.

use super::{PendingFrame, ReplayChunk};
use std::collections::VecDeque;

/// Bound both compressed storage and replay work per session. Retain a whole
/// GOP, or nothing: a suffix without its random-access picture is unusable.
pub(super) const MAX_REPLAY_CHUNKS: usize = 1024;
pub(super) const MAX_REPLAY_BYTES: usize = 32 * 1024 * 1024;

pub(super) fn remember(
    history: &mut Vec<ReplayChunk>,
    data: &[u8],
    timestamp: u64,
    keyframe: bool,
    surface: Option<u32>,
    expects_output: bool,
) -> bool {
    if keyframe {
        history.clear();
    } else if history.is_empty() {
        // Initial inter picture, or an overflowed GOP: wait for random access.
        return false;
    }
    let bytes = history.iter().try_fold(data.len(), |total, chunk| {
        total.checked_add(chunk.data.len())
    });
    if history.len() >= MAX_REPLAY_CHUNKS
        || bytes.is_none_or(|bytes| bytes > MAX_REPLAY_BYTES)
        || history.iter().any(|chunk| chunk.timestamp == timestamp)
    {
        history.clear();
        return false;
    }
    let mut retained = Vec::new();
    if retained.try_reserve_exact(data.len()).is_err() || history.try_reserve(1).is_err() {
        history.clear();
        return false;
    }
    retained.extend_from_slice(data);
    history.push(ReplayChunk {
        data: retained,
        timestamp,
        keyframe,
        surface,
        expects_output,
    });
    true
}

/// Retain decode order, including hidden reference pictures. A hole in the
/// published prefix cannot be repaired by simply omitting that access unit.
pub(super) fn drain_prefix<'a>(
    history: &'a [ReplayChunk],
    published: &VecDeque<u64>,
) -> Option<&'a [ReplayChunk]> {
    if !history.first()?.keyframe {
        return None;
    }
    let last = history
        .iter()
        .rposition(|chunk| published.contains(&chunk.timestamp))?;
    // A trailing hidden picture can be a reference for the next submission,
    // but published timestamps cannot prove whether it completed. Omitting it
    // would change references; replaying it could retire a pending hidden owner.
    if history[last + 1..]
        .iter()
        .any(|chunk| !chunk.expects_output)
    {
        return None;
    }
    let prefix = &history[..=last];
    if prefix
        .iter()
        .any(|chunk| chunk.expects_output && !published.contains(&chunk.timestamp))
    {
        return None;
    }
    Some(prefix)
}

/// The queued input must contain the entire retained GOP in decode order.
/// A keyframe alone is insufficient if later dependencies were consumed by
/// the failed incarnation and therefore disappeared from the OUTPUT queue.
pub(super) fn rebuild_is_complete(
    history: &[ReplayChunk],
    pending: &[PendingFrame],
    chunks: &[Vec<u8>],
) -> bool {
    if !history.first().is_some_and(|chunk| chunk.keyframe)
        || history.len() != chunks.len()
        || pending.len() != chunks.len()
    {
        return false;
    }
    history
        .iter()
        .zip(pending)
        .zip(chunks)
        .all(|((chunk, owner), data)| {
            chunk.timestamp == owner.timestamp
                && chunk.surface == Some(owner.surface)
                && chunk.expects_output == owner.expects_output
                && chunk.data == *data
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(timestamp: u64, keyframe: bool, expects_output: bool) -> ReplayChunk {
        ReplayChunk {
            data: vec![timestamp as u8],
            timestamp,
            keyframe,
            surface: Some(timestamp as u32),
            expects_output,
        }
    }

    #[test]
    fn chunk_limit_discards_whole_gop_and_waits_for_next_keyframe() {
        let mut history = Vec::new();
        for timestamp in 0..MAX_REPLAY_CHUNKS as u64 {
            assert!(remember(
                &mut history,
                &[1],
                timestamp,
                timestamp == 0,
                Some(1),
                true
            ));
        }
        assert_eq!(history.len(), MAX_REPLAY_CHUNKS);
        assert!(history[0].keyframe);
        assert!(!remember(
            &mut history,
            &[1],
            MAX_REPLAY_CHUNKS as u64,
            false,
            Some(1),
            true
        ));
        assert!(history.is_empty());
        assert!(!remember(&mut history, &[1], 2000, false, Some(1), true));
        assert!(history.is_empty());
        assert!(remember(&mut history, &[2], 2001, true, Some(1), true));
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].data, [2]);
    }

    #[test]
    fn byte_limit_retains_exact_budget_and_never_a_truncated_suffix() {
        let mut history = Vec::new();
        let full_budget = vec![1; MAX_REPLAY_BYTES];
        assert!(remember(&mut history, &full_budget, 0, true, Some(1), true));
        assert!(!remember(&mut history, &[2], 1, false, Some(1), true));
        assert!(history.is_empty());
        assert!(!remember(&mut history, &[3], 2, false, Some(1), true));
        assert!(remember(&mut history, &[4], 3, true, Some(1), true));
        assert_eq!(history[0].timestamp, 3);
    }

    #[test]
    fn reused_timestamps_invalidate_the_chain_instead_of_aliasing_output() {
        let mut history = Vec::new();
        assert!(remember(&mut history, &[1], 7, true, Some(1), true));
        assert!(!remember(&mut history, &[2], 7, false, Some(2), true));
        assert!(history.is_empty());
        assert!(!remember(&mut history, &[3], 8, false, Some(3), true));
    }

    #[test]
    fn long_gop_consumed_dependencies_cannot_be_rebuilt_from_queued_tail() {
        let mut history = Vec::new();
        for timestamp in 0..300 {
            assert!(remember(
                &mut history,
                &[timestamp as u8],
                timestamp,
                timestamp == 0,
                Some(timestamp as u32),
                timestamp % 3 != 1
            ));
        }
        let tail = PendingFrame {
            surface: 299,
            timestamp: 299,
            expects_output: true,
            direct_copy: false,
        };
        assert!(!rebuild_is_complete(
            &history,
            &[tail],
            &[vec![299_u64 as u8]]
        ));
        let mut owners: Vec<_> = history
            .iter()
            .map(|chunk| PendingFrame {
                surface: chunk.surface.unwrap(),
                timestamp: chunk.timestamp,
                expects_output: chunk.expects_output,
                direct_copy: false,
            })
            .collect();
        let queued: Vec<_> = history.iter().map(|chunk| chunk.data.clone()).collect();
        assert!(rebuild_is_complete(&history, &owners, &queued));
        // An older GOP owner still pending when a new keyframe starts must
        // not be silently dropped to make the queued/history lengths fit.
        owners.insert(
            0,
            PendingFrame {
                surface: 999,
                timestamp: 999,
                expects_output: true,
                direct_copy: false,
            },
        );
        assert!(!rebuild_is_complete(&history, &owners, &queued));
    }

    #[test]
    fn published_prefix_keeps_decode_order() {
        let history = vec![
            chunk(0, true, true),
            chunk(2, false, true),
            chunk(1, false, true),
        ];
        let prefix = drain_prefix(&history, &VecDeque::from([0, 1, 2])).unwrap();
        assert_eq!(
            prefix
                .iter()
                .map(|chunk| chunk.timestamp)
                .collect::<Vec<_>>(),
            [0, 2, 1]
        );
    }

    #[test]
    fn trailing_hidden_reference_without_completion_proof_fails_closed() {
        let history = vec![chunk(0, true, true), chunk(1, false, false)];
        assert!(drain_prefix(&history, &VecDeque::from([0])).is_none());
    }

    #[test]
    fn hidden_reference_access_units_are_not_filtered_out() {
        let history = vec![
            chunk(0, true, true),
            chunk(1, false, false),
            chunk(2, false, true),
        ];
        assert_eq!(
            drain_prefix(&history, &VecDeque::from([0, 2]))
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn a_missing_published_dependency_fails_closed() {
        let history = vec![
            chunk(0, true, true),
            chunk(1, false, true),
            chunk(2, false, true),
        ];
        assert!(drain_prefix(&history, &VecDeque::from([0, 2])).is_none());
    }

    #[test]
    fn a_truncated_gop_cannot_rebuild_references() {
        let history: Vec<_> = (1..65)
            .map(|timestamp| chunk(timestamp, false, true))
            .collect();
        let published = (1..65).collect();
        assert!(drain_prefix(&history, &published).is_none());
    }

    #[test]
    fn empty_or_unpublished_history_is_not_a_replay_prefix() {
        assert!(drain_prefix(&[], &VecDeque::new()).is_none());
        assert!(drain_prefix(&[chunk(0, true, true)], &VecDeque::new()).is_none());
    }

    #[test]
    fn unpublished_tail_is_not_replayed_as_a_completed_reference() {
        let history = vec![chunk(0, true, true), chunk(1, false, true)];
        assert_eq!(
            drain_prefix(&history, &VecDeque::from([0])).unwrap().len(),
            1
        );
    }

    #[test]
    fn rebuild_requires_all_reference_bytes_and_fifo_owners() {
        let history = vec![chunk(0, true, true), chunk(1, false, true)];
        let mut pending = vec![
            PendingFrame {
                surface: 0,
                timestamp: 0,
                expects_output: true,
                direct_copy: false,
            },
            PendingFrame {
                surface: 1,
                timestamp: 1,
                expects_output: true,
                direct_copy: false,
            },
        ];
        let mut data = vec![vec![0], vec![1]];
        assert!(rebuild_is_complete(&history, &pending, &data));
        data[1][0] = 2;
        assert!(!rebuild_is_complete(&history, &pending, &data));
        data[1][0] = 1;
        pending[1].surface = 0;
        assert!(!rebuild_is_complete(&history, &pending, &data));
        pending[1].surface = 1;
        pending[1].expects_output = false;
        assert!(!rebuild_is_complete(&history, &pending, &data));
    }

    #[test]
    fn a_queued_keyframe_does_not_cover_consumed_reference_pictures() {
        let history = vec![chunk(0, true, true), chunk(1, false, true)];
        let pending = vec![PendingFrame {
            surface: 0,
            timestamp: 0,
            expects_output: true,
            direct_copy: false,
        }];
        assert!(!rebuild_is_complete(&history, &pending, &[vec![0]]));
    }

    #[test]
    fn queued_inter_pictures_do_not_restore_a_missing_keyframe() {
        let history = vec![chunk(1, false, true)];
        let pending = vec![PendingFrame {
            surface: 1,
            timestamp: 1,
            expects_output: true,
            direct_copy: false,
        }];
        assert!(!rebuild_is_complete(&history, &pending, &[vec![1]]));
    }
}
