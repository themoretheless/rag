//! Pending worker requests: which answer each part of the app is waiting for.
//!
//! Every job sent to the worker carries a `seq`. The matching slot remembers
//! it, and an answer is applied only when its seq is still the one the slot
//! waits for — a newer request (or a reset) makes older answers stale.

/// One request slot: the seq of the job it waits for, if any.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Pending(Option<u64>);

impl Pending {
    /// Start waiting for `seq`, superseding any earlier request in this slot.
    pub(super) fn set(&mut self, seq: u64) {
        self.0 = Some(seq);
    }

    /// Stop waiting; a late answer for the old seq is then ignored.
    pub(super) fn clear(&mut self) {
        self.0 = None;
    }

    /// True while a request is in flight.
    pub(super) const fn is_some(self) -> bool {
        self.0.is_some()
    }

    /// True when nothing is in flight.
    pub(super) const fn is_none(self) -> bool {
        self.0.is_none()
    }

    /// Take the answer for `seq`. Returns true, and frees the slot, exactly
    /// when the slot was waiting for that seq; a stale seq changes nothing.
    pub(super) fn accept(&mut self, seq: u64) -> bool {
        if self.0 == Some(seq) {
            self.0 = None;
            true
        } else {
            false
        }
    }
}

/// Declares the slot set once, so `any()` can never miss a slot.
macro_rules! pending_slots {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {
        /// Every request slot of the app.
        #[derive(Debug, Default)]
        pub(super) struct PendingSlots {
            $($(#[$doc])* pub(super) $name: Pending,)+
        }

        impl PendingSlots {
            /// True while any request is in flight (drives the busy indicator
            /// and repaint scheduling).
            pub(super) fn any(&self) -> bool {
                [$(self.$name),+].iter().any(|slot| slot.is_some())
            }
        }
    };
}

pending_slots! {
    /// Topology load.
    graph,
    project_catalog,
    /// Wiki catalog list.
    catalog,
    /// Wiki article in pane A / pane B.
    page_a,
    page_b,
    backlinks,
    /// Document body for the selected graph node.
    content,
    expand,
    /// Wiki page save (not cancellable).
    save,
    activity,
    project_home,
    library,
    library_document,
    search,
    operations,
    jobs,
    /// Sync start/cancel, checkpoint or backup (not cancellable).
    operation_action,
    revisions,
    revision_snapshot,
    revision_diff,
    /// Revision restore (not cancellable).
    revision_restore,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_takes_only_the_seq_the_slot_waits_for() {
        let mut slot = Pending::default();
        assert!(slot.is_none());
        assert!(!slot.accept(1), "an idle slot accepts nothing");

        slot.set(1);
        assert!(slot.is_some());
        assert!(!slot.accept(2), "an unrelated seq is ignored");
        assert!(slot.is_some(), "a stale answer must not free the slot");
        assert!(slot.accept(1));
        assert!(slot.is_none());
        assert!(!slot.accept(1), "an answer is taken at most once");
    }

    #[test]
    fn newer_request_supersedes_and_clear_drops_the_answer() {
        let mut slot = Pending::default();
        slot.set(1);
        slot.set(2);
        assert!(!slot.accept(1), "superseded request is stale");
        assert!(slot.accept(2));

        slot.set(3);
        slot.clear();
        assert!(!slot.accept(3), "a cancelled request is ignored");
    }

    #[test]
    fn any_reports_every_slot() {
        let mut slots = PendingSlots::default();
        assert!(!slots.any());
        slots.revision_restore.set(9);
        assert!(slots.any());
        assert!(slots.revision_restore.accept(9));
        assert!(!slots.any());
        slots.graph.set(10);
        assert!(slots.any());
    }
}
