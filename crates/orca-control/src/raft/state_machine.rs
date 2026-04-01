//! Raft state machine backed by `ClusterStore`.

use std::io::Cursor;
use std::sync::Arc;

use openraft::storage::{RaftStateMachine, Snapshot};
use openraft::{
    Entry, EntryPayload, ErrorSubject, ErrorVerb, LogId, RaftSnapshotBuilder, SnapshotMeta,
    StorageError, StoredMembership,
};
use tokio::sync::Mutex;
use tracing::warn;

use super::type_config::OrcaTypeConfig;
use crate::store::{ClusterStore, RaftSnapshot};

type C = OrcaTypeConfig;

/// Helper to build a `StorageError` from an error reading the state machine.
fn sm_read_err(e: impl std::fmt::Display) -> StorageError<u64> {
    StorageError::from_io_error(
        ErrorSubject::StateMachine,
        ErrorVerb::Read,
        std::io::Error::other(e.to_string()),
    )
}

/// Raft state machine wrapping the persistent `ClusterStore`.
pub struct StateMachine {
    store: Arc<ClusterStore>,
    last_applied: Arc<Mutex<Option<LogId<u64>>>>,
    last_membership: Arc<Mutex<StoredMembership<u64, openraft::BasicNode>>>,
    snapshot: Arc<Mutex<Option<StoredSnapshot>>>,
}

/// A stored snapshot including metadata and serialized data.
struct StoredSnapshot {
    meta: SnapshotMeta<u64, openraft::BasicNode>,
    data: Vec<u8>,
}

impl StateMachine {
    pub fn new(store: Arc<ClusterStore>) -> Self {
        Self {
            store,
            last_applied: Arc::new(Mutex::new(None)),
            last_membership: Arc::new(Mutex::new(StoredMembership::default())),
            snapshot: Arc::new(Mutex::new(None)),
        }
    }

    async fn build_snapshot_impl(&self) -> Result<Snapshot<C>, StorageError<u64>> {
        let snap = self.store.snapshot().map_err(|e| sm_read_err(&e))?;
        let data = serde_json::to_vec(&snap).map_err(|e| sm_read_err(&e))?;

        let last_applied = *self.last_applied.lock().await;
        let last_membership = self.last_membership.lock().await.clone();

        let meta = SnapshotMeta {
            last_log_id: last_applied,
            last_membership,
            snapshot_id: format!("snap-{}", chrono::Utc::now().timestamp_millis()),
        };

        {
            let mut s = self.snapshot.lock().await;
            *s = Some(StoredSnapshot {
                meta: meta.clone(),
                data: data.clone(),
            });
        }

        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

impl RaftSnapshotBuilder<C> for Arc<StateMachine> {
    async fn build_snapshot(&mut self) -> Result<Snapshot<C>, StorageError<u64>> {
        self.build_snapshot_impl().await
    }
}

impl RaftStateMachine<C> for StateMachine {
    type SnapshotBuilder = Arc<Self>;

    async fn applied_state(
        &mut self,
    ) -> Result<
        (
            Option<LogId<u64>>,
            StoredMembership<u64, openraft::BasicNode>,
        ),
        StorageError<u64>,
    > {
        let applied = *self.last_applied.lock().await;
        let membership = self.last_membership.lock().await.clone();
        Ok((applied, membership))
    }

    async fn apply<I>(&mut self, entries: I) -> Result<Vec<()>, StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<C>> + Send,
        I::IntoIter: Send,
    {
        let mut results = Vec::new();
        for entry in entries {
            *self.last_applied.lock().await = Some(entry.log_id);

            match &entry.payload {
                EntryPayload::Normal(raft_entry) => {
                    if let Err(e) = self.store.apply(raft_entry) {
                        warn!("Failed to apply entry: {e}");
                    }
                }
                EntryPayload::Membership(m) => {
                    *self.last_membership.lock().await =
                        StoredMembership::new(Some(entry.log_id), m.clone());
                }
                EntryPayload::Blank => {}
            }
            results.push(());
        }
        Ok(results)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        Arc::new(Self {
            store: self.store.clone(),
            last_applied: self.last_applied.clone(),
            last_membership: self.last_membership.clone(),
            snapshot: self.snapshot.clone(),
        })
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<u64>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<u64, openraft::BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<u64>> {
        let data = snapshot.into_inner();
        let snap: RaftSnapshot = serde_json::from_slice(&data).map_err(|e| sm_read_err(&e))?;

        self.store
            .restore_from_snapshot(&snap)
            .map_err(|e| sm_read_err(&e))?;

        *self.last_applied.lock().await = meta.last_log_id;
        *self.last_membership.lock().await = meta.last_membership.clone();
        *self.snapshot.lock().await = Some(StoredSnapshot {
            meta: meta.clone(),
            data,
        });
        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<Snapshot<C>>, StorageError<u64>> {
        let guard = self.snapshot.lock().await;
        Ok(guard.as_ref().map(|s| Snapshot {
            meta: s.meta.clone(),
            snapshot: Box::new(Cursor::new(s.data.clone())),
        }))
    }
}
