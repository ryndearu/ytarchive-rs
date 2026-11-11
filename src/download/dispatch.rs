//! Handing out fragment sequence numbers to download workers.

use std::collections::HashSet;

use crate::log_debug;

/// Hands out sequence numbers to workers, never running more than one
/// fragment past the newest known one.
#[derive(Debug)]
pub(super) struct Dispatch {
    pub next: u64,
    pub max_seq: Option<u64>,
    pub end_seq: Option<u64>,
    pub in_flight: usize,
    /// Already stored (SQLite resume).
    pub skip: HashSet<u64>,
}

pub(super) enum Take {
    Seq(u64, Option<u64>),
    Wait,
    Done,
}

impl Dispatch {
    pub fn take(&mut self, live: bool) -> Take {
        while self.skip.remove(&self.next) {
            self.next += 1;
        }
        if self.end_seq.is_some_and(|end| self.next >= end) {
            log_debug!("Reached the maximum duration specified by --capture-duration.");
            return Take::Done;
        }
        if let Some(max) = self.max_seq {
            if !live && self.next >= max {
                return Take::Done;
            }
            if self.next > max + 1 && self.in_flight > 0 {
                return Take::Wait;
            }
        }
        let seq = self.next;
        self.next += 1;
        self.in_flight += 1;
        Take::Seq(seq, self.max_seq)
    }

    pub fn complete(&mut self, head: Option<u64>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(head_seq) = head {
            self.max_seq = Some(self.max_seq.map_or(head_seq, |max| max.max(head_seq)));
        }
    }
}
