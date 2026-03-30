//! HTTP-based Raft RPC transport using reqwest.

use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::RPCOption;
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, RaftNetwork, RaftNetworkFactory};

use super::type_config::OrcaTypeConfig;

type C = OrcaTypeConfig;

/// Factory that creates HTTP network connections to Raft peers.
pub struct NetworkFactory {
    client: reqwest::Client,
}

impl Default for NetworkFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkFactory {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl RaftNetworkFactory<C> for NetworkFactory {
    type Network = NetworkConnection;

    async fn new_client(&mut self, _target: u64, node: &BasicNode) -> Self::Network {
        NetworkConnection {
            addr: node.addr.clone(),
            client: self.client.clone(),
        }
    }
}

/// A single HTTP connection to a Raft peer node.
pub struct NetworkConnection {
    addr: String,
    client: reqwest::Client,
}

impl NetworkConnection {
    fn url(&self, path: &str) -> String {
        format!("http://{}/raft/{}", self.addr, path)
    }
}

impl RaftNetwork<C> for NetworkConnection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<C>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let resp = self
            .client
            .post(self.url("append"))
            .json(&rpc)
            .send()
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let body = resp
            .json()
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;
        Ok(body)
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<C>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, InstallSnapshotError>>,
    > {
        let resp = self
            .client
            .post(self.url("snapshot"))
            .json(&rpc)
            .send()
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let body: InstallSnapshotResponse<u64> = resp
            .json()
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;
        Ok(body)
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let resp = self
            .client
            .post(self.url("vote"))
            .json(&rpc)
            .send()
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let body = resp
            .json()
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;
        Ok(body)
    }
}
