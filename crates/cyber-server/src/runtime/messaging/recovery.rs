//! Startup reconciliation of half-recorded messages (design sections 5.2 and 6). The
//! target commit is authoritative and the sender's record follows it; a crash between the
//! two leaves an `in` record without its `out` record, or a release or drop the sender's
//! `out` record does not show yet, which recovery appends before external admission opens.

use crate::runtime::{Runtime, RuntimeError};

use super::records;

impl Runtime {
    /// Must run once during application construction, before external admission (next
    /// to `recover_jobs`). Returns the number of sender records appended.
    pub async fn recover_messages(&self) -> Result<usize, RuntimeError> {
        self.complete_sent(None).await
    }

    /// Append the `out` records missing for `message_id`, or for every message: first
    /// the sender records of sends, then the releases and drops they do not show yet.
    pub(crate) async fn complete_sent(
        &self,
        message_id: Option<&str>,
    ) -> Result<usize, RuntimeError> {
        let mut count = 0;
        for outcomes in [false, true] {
            let id = message_id.map(str::to_owned);
            let missing = self.inner.store.read(move |db| {
                Ok(if outcomes {
                    records::unrecorded_outcomes(db, id.as_deref())?
                } else {
                    records::unrecorded(db, id.as_deref())?
                })
            })?;
            count += missing.len();
            for unrecorded in missing {
                let (sender, record) = unrecorded.out_record();
                self.record_message(&sender, record).await?;
            }
        }
        Ok(count)
    }
}
