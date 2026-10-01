//! Reference-chain validation before destructive decoder transitions.
//!
//! Parameter sets describe pictures; they do not recreate decoded references.
//! The bounded history must retain a random-access picture and every following
//! dependency. Recovery fails closed when that evidence is missing.

use super::{PendingFrame, ReplayChunk};
use std::collections::VecDeque;

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
            },
            PendingFrame {
                surface: 1,
                timestamp: 1,
                expects_output: true,
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
        }];
        assert!(!rebuild_is_complete(&history, &pending, &[vec![1]]));
    }
}
