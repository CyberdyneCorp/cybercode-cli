use cyber_store::Store;
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::sync::Arc;

use super::{ComputerError, FrameInfo, Receipt, Result, UnknownOperation};

pub struct Journal(pub Arc<Store>);
impl Journal {
    pub fn recover(&self) -> Result<()> {
        self.0.transaction(|tx| {
            tx.execute(
                "UPDATE computer_operation SET status='unknown' WHERE status='intent'",
                [],
            )?;
            Ok(())
        })?;
        Ok(())
    }
    pub fn replay(
        &self,
        session: &str,
        operation: &str,
        fingerprint: &str,
    ) -> Result<Option<Receipt>> {
        let (session, operation) = (session.to_owned(), operation.to_owned());
        let lookup = operation.clone();
        let row = self.0.read(move |db| db.query_row("SELECT session_id,fingerprint,status,receipt,action FROM computer_operation WHERE operation_key=?1", [&lookup], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,String>(4)?))).optional().map_err(Into::into))?;
        let Some((owner, digest, status, receipt, action)) = row else {
            return Ok(None);
        };
        if owner != session || digest != fingerprint {
            return Err(ComputerError::Conflict);
        }
        let mut receipt = receipt
            .and_then(|value| serde_json::from_str::<Receipt>(&value).ok())
            .unwrap_or(Receipt {
                operation_id: operation,
                action,
                outcome: if status == "intent" {
                    "unknown".into()
                } else {
                    status
                },
                replayed: false,
                frame: None,
            });
        receipt.replayed = true;
        Ok(Some(receipt))
    }
    pub fn begin(
        &self,
        session: &str,
        operation: &str,
        action: &str,
        fingerprint: &str,
        audit: Value,
        now: i64,
    ) -> Result<()> {
        let (session, operation, action, fingerprint) = (
            session.to_owned(),
            operation.to_owned(),
            action.to_owned(),
            fingerprint.to_owned(),
        );
        let inserted = self.0.transaction(move |tx| Ok(tx.execute("INSERT OR IGNORE INTO computer_operation(operation_key,session_id,action,fingerprint,audit,status,started_ms) VALUES(?1,?2,?3,?4,?5,'intent',?6)", rusqlite::params![operation,session,action,fingerprint,audit.to_string(),now])?))?;
        if inserted != 1 {
            return Err(ComputerError::Conflict);
        }
        Ok(())
    }
    pub fn settle(&self, receipt: &Receipt, now: i64) -> Result<()> {
        let receipt = receipt.clone();
        let changed = self.0.transaction(move |tx| Ok(tx.execute("UPDATE computer_operation SET status=?2,settled_ms=?3,receipt=?4 WHERE operation_key=?1 AND status='intent'", rusqlite::params![receipt.operation_id,receipt.outcome,now,serde_json::to_string(&receipt).map_err(|_|cyber_store::StoreError::Unavailable("computer receipt serialization failed".into()))?])?))?;
        if changed != 1 {
            return Err(ComputerError::Conflict);
        }
        Ok(())
    }
    pub fn unknown(&self, session: &str) -> Result<Vec<UnknownOperation>> {
        self.unknown_for(Some(session.to_owned()))
    }
    pub fn unknown_all(&self) -> Result<Vec<UnknownOperation>> {
        self.unknown_for(None)
    }
    fn unknown_for(&self, session: Option<String>) -> Result<Vec<UnknownOperation>> {
        Ok(self.0.read(move|db| {
            let mut query=db.prepare("SELECT operation_key,action,started_ms,session_id FROM computer_operation WHERE (?1 IS NULL OR session_id=?1) AND action!='screenshot' AND status IN ('intent','unknown') ORDER BY operation_key")?;
            query.query_map([session],|row|Ok(UnknownOperation {operation_id:row.get(0)?,action:row.get(1)?,started_ms:row.get(2)?,session_id:row.get(3)?}))?.collect::<std::result::Result<Vec<_>,_>>().map_err(Into::into)
        })?)
    }
    pub fn acknowledge(&self, expected: &[UnknownOperation]) -> Result<()> {
        let expected = expected
            .iter()
            .map(|row| row.operation_id.clone())
            .collect::<Vec<_>>();
        let okay=self.0.transaction(move|tx| {
            let mut query=tx.prepare("SELECT operation_key FROM computer_operation WHERE action!='screenshot' AND status IN ('intent','unknown') ORDER BY operation_key")?;
            let actual=query.query_map([],|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            if actual!=expected {return Ok(false);}
            tx.execute("UPDATE computer_operation SET status='unknown_acknowledged' WHERE action!='screenshot' AND status IN ('intent','unknown')",[])?;Ok(true)
        })?;
        if !okay {
            return Err(ComputerError::Conflict);
        }
        Ok(())
    }
    pub fn frame(
        &self,
        info: &FrameInfo,
        target: &cyber_computer::backend::NativeTarget,
        expiry: Option<i64>,
    ) -> Result<()> {
        let info = info.clone();
        let target = serde_json::to_string(target).map_err(|_| ComputerError::Storage)?;
        self.0.transaction(move |tx| { tx.execute("INSERT INTO computer_frame(id,session_id,sha256,metadata,native_target,bytes,created_ms,expires_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",rusqlite::params![info.id,info.session_id,info.sha256,serde_json::to_string(&info).map_err(|_|cyber_store::StoreError::Unavailable("computer frame serialization failed".into()))?,target,info.bytes as i64,info.created_ms,expiry])?; Ok(()) })?;
        Ok(())
    }
    pub fn read_frame(
        &self,
        session: &str,
        frame: &str,
        now: i64,
        retention_days: u64,
    ) -> Result<(FrameInfo, cyber_computer::backend::NativeTarget)> {
        let (session, frame) = (session.to_owned(), frame.to_owned());
        let cutoff = if retention_days == 0 {
            i64::MIN
        } else {
            now.saturating_sub(
                retention_days
                    .saturating_mul(86_400_000)
                    .min(i64::MAX as u64) as i64,
            )
        };
        let row = self.0.read(move |db| db.query_row("SELECT metadata,native_target FROM computer_frame WHERE id=?1 AND session_id=?2 AND expired=0 AND ready=1 AND created_ms>?3",rusqlite::params![frame,session,cutoff],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional().map_err(Into::into))?;
        let Some((info, target)) = row else {
            return Err(ComputerError::FrameExpired);
        };
        Ok((
            serde_json::from_str(&info).map_err(|_| ComputerError::Storage)?,
            serde_json::from_str(&target).map_err(|_| ComputerError::Storage)?,
        ))
    }
    pub fn latest_frame(&self, session: &str, retention_days: u64) -> Result<Option<FrameInfo>> {
        let session = session.to_owned();
        let cutoff = if retention_days == 0 {
            i64::MIN
        } else {
            super::wall_ms().saturating_sub(
                retention_days
                    .saturating_mul(86_400_000)
                    .min(i64::MAX as u64) as i64,
            )
        };
        let metadata=self.0.read(move|db|db.query_row("SELECT metadata FROM computer_frame WHERE session_id=?1 AND expired=0 AND ready=1 AND created_ms>?2 ORDER BY created_ms DESC,id DESC LIMIT 1",rusqlite::params![session,cutoff],|row|row.get::<_,String>(0)).optional().map_err(Into::into))?;
        metadata
            .map(|metadata| serde_json::from_str(&metadata).map_err(|_| ComputerError::Storage))
            .transpose()
    }
    pub fn stored_bytes(&self) -> Result<u64> {
        Ok(self.0.read(|db| {
            db.query_row(
                "SELECT COALESCE(SUM(bytes),0) FROM computer_frame WHERE expired=0",
                [],
                |row| row.get::<_, i64>(0).map(|value| value.max(0) as u64),
            )
            .map_err(Into::into)
        })?)
    }
}
