//! The taint of a run (`permissions-modes` → Untrusted-content taint): the `untrusted`
//! records promoted into context since the last prompt with source `user`. The runtime
//! folds it from `session.tool.settled.1` and `session.prompt.admitted.1`, so a restart
//! rebuilds it and compaction never changes it.

use serde::Serialize;

use super::{Scan, SignalKind, TaintSetting, UntrustedRecord};

/// At most this many records with signals, and as many without, are kept per run: the
/// taint and its evidence need the first ones, not all of them.
const KEPT: usize = 16;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunTaint {
    /// Records in the order they were promoted.
    records: Vec<UntrustedRecord>,
}

impl RunTaint {
    /// Note a record promoted into context.
    pub fn note(&mut self, record: &UntrustedRecord) {
        let signaled = record.has_signals();
        let kept = self
            .records
            .iter()
            .filter(|kept| kept.has_signals() == signaled)
            .count();
        if kept < KEPT {
            self.records.push(record.clone());
        }
    }

    /// A prompt with source `user` was promoted: the run starts over untainted.
    pub fn clear(&mut self) {
        self.records.clear();
    }

    /// The records that taint the run under `setting`, oldest first; empty when the run
    /// is not tainted. `signal` counts records with signals, `any` every record (each is
    /// a framed result, or input that had hits).
    pub fn sources(&self, setting: TaintSetting) -> Vec<UntrustedRecord> {
        let keep = |record: &&UntrustedRecord| match setting {
            TaintSetting::Off => false,
            TaintSetting::Signal => record.has_signals(),
            TaintSetting::Any => true,
        };
        self.records.iter().filter(keep).cloned().collect()
    }

    /// The record a child spawned now inherits, and a tainted child hands its parent with
    /// its result: the first tainting record's source and origin with the union of all
    /// kinds. Records with signals come first, so the inherited record has signals
    /// exactly when this run has; a run with only signal-free framed results hands on a
    /// record without signals, which taints only under `taint: "any"`.
    pub fn inherited(&self) -> Option<UntrustedRecord> {
        let signaled = self.sources(TaintSetting::Signal);
        summary(if signaled.is_empty() {
            &self.records
        } else {
            &signaled
        })
    }
}

/// One record as auto classifier evidence (`permissions-modes` → Taint context for the
/// auto classifier).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaintSource {
    pub source: String,
    pub origin: String,
    pub kinds: Vec<SignalKind>,
    pub excerpts: Vec<String>,
}

impl TaintSource {
    pub fn of(record: &UntrustedRecord) -> Self {
        Self {
            source: record.source.clone(),
            origin: record.origin.clone(),
            kinds: record.kinds(),
            excerpts: record.signals.iter().map(|s| s.excerpt.clone()).collect(),
        }
    }
}

fn summary(records: &[UntrustedRecord]) -> Option<UntrustedRecord> {
    let first = records.first()?;
    let scan = records.iter().fold(Scan::default(), |scan, record| {
        scan.merge(Scan {
            signals: record.signals.clone(),
            count: record.signal_count,
        })
    });
    Some(UntrustedRecord {
        source: first.source.clone(),
        origin: first.origin.clone(),
        detector_version: first.detector_version,
        signals: scan.signals,
        signal_count: scan.count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::untrusted::Signal;

    fn record(source: &str, origin: &str, kinds: &[SignalKind]) -> UntrustedRecord {
        let signals: Vec<Signal> = kinds
            .iter()
            .map(|&kind| Signal {
                kind,
                excerpt: format!("{} excerpt", kind.as_str()),
            })
            .collect();
        let count = signals.len() as u32;
        UntrustedRecord::new(source, origin, Scan { signals, count })
    }

    #[test]
    fn sources_follow_the_setting_and_clear_starts_over() {
        let mut taint = RunTaint::default();
        let clean = record("websearch", "search:brave", &[]);
        let flagged = record("webfetch", "https://x.example/a", &[SignalKind::ExfilUrl]);
        taint.note(&clean);
        assert!(taint.sources(TaintSetting::Signal).is_empty());
        assert_eq!(
            taint.sources(TaintSetting::Any),
            std::slice::from_ref(&clean)
        );
        taint.note(&flagged);
        assert_eq!(
            taint.sources(TaintSetting::Signal),
            std::slice::from_ref(&flagged)
        );
        assert_eq!(taint.sources(TaintSetting::Any), [clean, flagged]);
        assert!(taint.sources(TaintSetting::Off).is_empty());
        taint.clear();
        assert_eq!(taint, RunTaint::default());
    }

    #[test]
    fn records_are_bounded_per_class() {
        let mut taint = RunTaint::default();
        for i in 0..40 {
            taint.note(&record("mcp", &format!("mcp:s/t{i}"), &[]));
            taint.note(&record(
                "webfetch",
                &format!("https://x/{i}"),
                &[SignalKind::RoleMarkup],
            ));
        }
        assert_eq!(taint.sources(TaintSetting::Signal).len(), KEPT);
        assert_eq!(taint.sources(TaintSetting::Any).len(), 2 * KEPT);
    }

    /// The inherited record names the first tainting result and unions the kinds; a run
    /// with only signal-free results hands on a record without signals.
    #[test]
    fn inherited_record_summarizes_the_tainting_records() {
        let mut taint = RunTaint::default();
        assert_eq!(taint.inherited(), None);
        taint.note(&record("mcp", "mcp:docs/search", &[]));
        let inherited = taint.inherited().unwrap();
        assert_eq!(inherited.origin, "mcp:docs/search");
        assert!(!inherited.has_signals());
        taint.note(&record(
            "webfetch",
            "https://x.example/a",
            &[SignalKind::SecretAccess],
        ));
        taint.note(&record(
            "mcp",
            "mcp:docs/read",
            &[SignalKind::OverrideInstructions],
        ));
        let inherited = taint.inherited().unwrap();
        assert_eq!(
            (inherited.source.as_str(), inherited.origin.as_str()),
            ("webfetch", "https://x.example/a")
        );
        assert_eq!(
            inherited.kinds(),
            [SignalKind::OverrideInstructions, SignalKind::SecretAccess]
        );
        assert_eq!(inherited.signal_count, 2);
        let evidence = TaintSource::of(&inherited);
        assert_eq!(evidence.excerpts.len(), 2);
    }
}
