//! In-memory Raft log storage for Orca.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::storage::{LogFlushed, LogState, RaftLogStorage};
use openraft::{Entry, LogId, RaftLogReader, StorageError, Vote};
use tokio::sync::Mutex;

use super::type_config::OrcaTypeConfig;

type C = OrcaTypeConfig;

/// Shared inner state of the log store, guarded by a mutex.
struct Inner {
    last_purged: Option<LogId<u64>>,
    log: BTreeMap<u64, Entry<C>>,
    vote: Option<Vote<u64>>,
}

/// In-memory Raft log store.
///
/// Entries live in a `BTreeMap<u64, Entry>` behind an `Arc<Mutex<>>`.
/// Sufficient for single-node and small clusters; for durable setups,
/// swap in a redb-backed implementation.
#[derive(Clone)]
pub struct LogStore {
    inner: Arc<Mutex<Inner>>,
}

impl Default for LogStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LogStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                last_purged: None,
                log: BTreeMap::new(),
                vote: None,
            })),
        }
    }
}

impl RaftLogReader<C> for LogStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<C>>, StorageError<u64>> {
        let inner = self.inner.lock().await;
        let entries = inner.log.range(range).map(|(_, v)| v.clone()).collect();
        Ok(entries)
    }
}

impl RaftLogStorage<C> for LogStore {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> Result<LogState<C>, StorageError<u64>> {
        let inner = self.inner.lock().await;
        let last_log_id = inner
            .log
            .last_key_value()
            .map(|(_, e)| e.log_id)
            .or(inner.last_purged);
        Ok(LogState {
            last_purged_log_id: inner.last_purged,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.lock().await;
        inner.vote = Some(*vote);
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        let inner = self.inner.lock().await;
        Ok(inner.vote)
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<C>,
    ) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<C>> + Send,
        I::IntoIter: Send,
    {
        let mut inner = self.inner.lock().await;
        for entry in entries {
            let idx = entry.log_id.index;
            inner.log.insert(idx, entry);
        }
        // In-memory store is immediately "flushed".
        callback.log_io_completed(Ok(()));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.lock().await;
        let to_remove: Vec<u64> = inner.log.range(log_id.index..).map(|(k, _)| *k).collect();
        for k in to_remove {
            inner.log.remove(&k);
        }
        Ok(())
    }

    async fn purge(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.lock().await;
        let to_remove: Vec<u64> = inner.log.range(..=log_id.index).map(|(k, _)| *k).collect();
        for k in to_remove {
            inner.log.remove(&k);
        }
        inner.last_purged = Some(log_id);
        Ok(())
    }
}
