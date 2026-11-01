//! Diagnostics of the objects a simulator is asked for (#1858).
//!
//! An `ObjectUpdateCached` miss is asked for again with
//! RequestMultipleObjects; neither the Legacy Viewer
//! (LLViewerRegion::requestCacheMisses) nor Aurora tracks the answer. In a
//! busy region, avatars were shown without any of their attachments after
//! 60 s, and nothing told whether the objects were never asked for, asked
//! for and never answered, or answered and dropped here. This watch only
//! counts and logs: the requests themselves stay the Legacy Viewer's.
//!
//! - Requested objects whose full update has not come after `LATE` are
//!   reported, once each, then forgotten (`report`).
//! - Object update blocks the parser rejects are counted and the first few
//!   described (`rejected`): they were dropped without a trace, while the
//!   packet that held them is acknowledged.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// A requested object not answered after this long is reported.
pub(super) const LATE: Duration = Duration::from_secs(10);
/// How often the watch reports.
pub(super) const REPORT_EVERY: Duration = Duration::from_secs(10);
/// Rejected blocks described in the log, per session (then only counted).
const DESCRIBED: u32 = 20;

#[derive(Default)]
pub(super) struct ObjectWatch {
    /// Per simulator: local id -> when it was asked for.
    asked: HashMap<SocketAddr, HashMap<u32, Instant>>,
    last_report: Option<Instant>,
    /// Asked for, then answered, since the start (for the report).
    total_asked: u64,
    total_late: u64,
    rejected: u64,
    described: u32,
}

/// What `report` found: objects that are late, per simulator.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Late {
    pub addr: SocketAddr,
    pub count: usize,
    /// A few of their local ids, to find them in the log.
    pub sample: Vec<u32>,
}

impl ObjectWatch {
    pub fn asked(&mut self, addr: SocketAddr, ids: &[u32], now: Instant) {
        let pending = self.asked.entry(addr).or_default();
        for &id in ids {
            pending.entry(id).or_insert(now);
        }
        self.total_asked += ids.len() as u64;
    }

    /// Full or compressed updates of these objects came from `addr`.
    pub fn received(&mut self, addr: SocketAddr, ids: impl IntoIterator<Item = u32>) {
        if let Some(pending) = self.asked.get_mut(&addr) {
            for id in ids {
                pending.remove(&id);
            }
        }
    }

    pub fn forget(&mut self, addr: SocketAddr) {
        self.asked.remove(&addr);
    }

    /// The objects asked for more than `LATE` ago and still not received,
    /// each reported once (then dropped from the watch). None between
    /// reports or when nothing is late.
    pub fn report(&mut self, now: Instant) -> Option<Vec<Late>> {
        if self.last_report.is_some_and(|t| now.duration_since(t) < REPORT_EVERY) {
            return None;
        }
        self.last_report = Some(now);
        let mut out = Vec::new();
        for (&addr, pending) in &mut self.asked {
            let mut late: Vec<u32> = pending
                .iter()
                .filter(|(_, at)| now.duration_since(**at) >= LATE)
                .map(|(&id, _)| id)
                .collect();
            if late.is_empty() {
                continue;
            }
            late.sort_unstable();
            for id in &late {
                pending.remove(id);
            }
            self.total_late += late.len() as u64;
            out.push(Late {
                addr,
                count: late.len(),
                sample: late.into_iter().take(8).collect(),
            });
        }
        out.sort_by_key(|l| l.addr);
        (!out.is_empty()).then_some(out)
    }

    pub fn totals(&self) -> (u64, u64) {
        (self.total_asked, self.total_late)
    }

    /// A block the parser rejected; true when it should be described.
    pub fn rejected(&mut self) -> bool {
        self.rejected += 1;
        let describe = self.described < DESCRIBED;
        if describe {
            self.described += 1;
        }
        describe
    }

    pub fn rejected_count(&self) -> u64 {
        self.rejected
    }
}

/// What can be read of a rejected compressed block: its id, local id, PCode
/// and update flags (the fields before anything optional), and its size.
pub(super) fn describe_compressed(data: &[u8], update_flags: u32) -> String {
    let id = data
        .get(0..16)
        .and_then(|b| uuid::Uuid::from_slice(b).ok())
        .map_or_else(|| "?".to_string(), |u| u.to_string());
    let local = data
        .get(16..20)
        .map_or_else(|| "?".to_string(), |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_string());
    let pcode = data.get(20).map_or_else(|| "?".to_string(), |p| p.to_string());
    format!(
        "{} bytes, object {id}, local id {local}, pcode {pcode}, update flags {update_flags:#x}",
        data.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, n], 13000))
    }

    #[test]
    fn answered_objects_are_not_reported_and_late_ones_once() {
        let t0 = Instant::now();
        let mut w = ObjectWatch::default();
        w.asked(addr(1), &[1, 2, 3], t0);
        w.received(addr(1), [2]);
        // too early: nothing late yet
        assert_eq!(w.report(t0 + Duration::from_secs(1)), None);
        // late, after the report interval
        let late = w.report(t0 + LATE + REPORT_EVERY).expect("late objects");
        assert_eq!(
            late,
            vec![Late {
                addr: addr(1),
                count: 2,
                sample: vec![1, 3]
            }]
        );
        // reported once, then forgotten
        assert_eq!(w.report(t0 + LATE + REPORT_EVERY * 2), None);
        assert_eq!(w.totals(), (3, 2));
    }

    #[test]
    fn a_closed_simulator_is_forgotten() {
        let t0 = Instant::now();
        let mut w = ObjectWatch::default();
        w.asked(addr(1), &[1], t0);
        w.asked(addr(2), &[1], t0);
        w.forget(addr(1));
        let late = w.report(t0 + LATE + REPORT_EVERY).expect("late objects");
        assert_eq!(late.len(), 1);
        assert_eq!(late[0].addr, addr(2));
    }

    #[test]
    fn only_the_first_rejected_blocks_are_described() {
        let mut w = ObjectWatch::default();
        let described = (0..30).filter(|_| w.rejected()).count();
        assert_eq!(described, DESCRIBED as usize);
        assert_eq!(w.rejected_count(), 30);
    }

    #[test]
    fn a_rejected_block_names_what_it_can() {
        let mut data = vec![0u8; 22];
        data[16..20].copy_from_slice(&1234u32.to_le_bytes());
        data[20] = 9;
        let d = describe_compressed(&data, 0x20);
        assert!(
            d.contains("22 bytes") && d.contains("local id 1234") && d.contains("pcode 9") && d.contains("0x20"),
            "{d}"
        );
        assert!(describe_compressed(&[1, 2], 0).contains("local id ?"));
    }
}
