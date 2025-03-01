use crate::types::{AddItemError, DeleteItemError, PeerData, QueryItemError, ScanTableError};
use async_trait::async_trait;
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::{config::Region, Client};
use chrono::{DateTime, Days, Duration, Utc};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio_rusqlite::Connection;
use tokio_stream::StreamExt;
use tracing::info;

#[async_trait]
pub trait PeerDB: Send + Sync {
    async fn add_peer(&self, peer_data: PeerData, ttl: Option<i64>) -> Result<(), AddItemError>;
    async fn all_peers(&self, page_size: Option<i32>) -> Result<Vec<PeerData>, ScanTableError>;
    async fn node_by_id(&self, id: String) -> Result<Option<Vec<PeerData>>, QueryItemError>;
    async fn node_by_ip(&self, ip: String) -> Result<Option<Vec<PeerData>>, QueryItemError>;
}

#[derive(Clone)]
pub struct AwsPeerDB {
    client: Client,
}

impl AwsPeerDB {
    pub async fn new() -> Self {
        let region_provider =
            RegionProviderChain::default_provider().or_else(Region::new("us-west-2"));
        let shared_config = aws_config::from_env().region(region_provider).load().await;
        let client = Client::new(&shared_config);

        AwsPeerDB { client }
    }

    pub async fn all_last_peers(
        &self,
        last_seen: String,
        page_size: Option<i32>,
    ) -> Result<Vec<PeerData>, ScanTableError> {
        let page_size = page_size.unwrap_or(1000);
        let results: Result<Vec<_>, _> = self
            .client
            .scan()
            .table_name("eth-peer-data")
            .filter_expression("last_seen > :last_seen_parameter")
            .expression_attribute_values(
                ":last_seen_parameter",
                AttributeValue::S(last_seen.clone()),
            )
            .limit(page_size)
            .into_paginator()
            .items()
            .send()
            .collect()
            .await;
        match results {
            Ok(peers) => peers.iter().map(|peer| Ok(peer.into())).collect(),
            Err(err) => Err(err.into()),
        }
    }
}

#[async_trait]
impl PeerDB for AwsPeerDB {
    async fn add_peer(&self, peer_data: PeerData, ttl: Option<i64>) -> Result<(), AddItemError> {
        let capabilities = peer_data
            .capabilities
            .iter()
            .map(|cap| AttributeValue::S(cap.clone()))
            .collect();
        let peer_id = AttributeValue::S(peer_data.id);
        let peer_ip = AttributeValue::S(peer_data.address);
        let client_version = AttributeValue::S(peer_data.client_version);
        let enode_url = AttributeValue::S(peer_data.enode_url);
        let port = AttributeValue::N(peer_data.tcp_port.to_string()); // numbers are sent over the network as string
        let chain = AttributeValue::S(peer_data.chain);
        let genesis_hash = AttributeValue::S(peer_data.genesis_block_hash);
        let best_block = AttributeValue::S(peer_data.best_block);
        let total_difficulty = AttributeValue::S(peer_data.total_difficulty);
        let country = AttributeValue::S(peer_data.country);
        let city = AttributeValue::S(peer_data.city);
        let last_seen = AttributeValue::S(peer_data.last_seen);
        let region_source = AttributeValue::S(self.client.config().region().unwrap().to_string());
        let ttl = AttributeValue::N(ttl.unwrap().to_string());
        let capabilities = AttributeValue::L(capabilities);
        let eth_version = AttributeValue::N(peer_data.eth_version.to_string());

        match self
            .client
            .put_item()
            .table_name("eth-peer-data")
            .item("peer-id", peer_id)
            .item("peer-ip", peer_ip)
            .item("client_version", client_version)
            .item("enode_url", enode_url)
            .item("port", port)
            .item("chain", chain)
            .item("country", country)
            .item("city", city)
            .item("capabilities", capabilities)
            .item("eth_version", eth_version)
            .item("last_seen", last_seen)
            .item("source_region", region_source)
            .item("genesis_block_hash", genesis_hash)
            .item("best_block", best_block)
            .item("total_difficulty", total_difficulty)
            .item("ttl", ttl)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn all_peers(&self, page_size: Option<i32>) -> Result<Vec<PeerData>, ScanTableError> {
        let page_size = page_size.unwrap_or(1000);
        let cutoff = Utc::now()
            .checked_sub_signed(Duration::hours(24))
            .unwrap()
            .to_string();
        let results: Result<Vec<_>, _> = self
            .client
            .scan()
            .filter_expression("last_seen > :last_seen_parameter")
            .expression_attribute_values(":last_seen_parameter", AttributeValue::S(cutoff.clone()))
            .table_name("eth-peer-data")
            .limit(page_size)
            .into_paginator()
            .items()
            .send()
            .collect()
            .await;

        match results {
            Ok(peers) => peers.iter().map(|peer| Ok(peer.into())).collect(),
            Err(err) => Err(err.into()),
        }
    }

    async fn node_by_id(&self, id: String) -> Result<Option<Vec<PeerData>>, QueryItemError> {
        let results = self
            .client
            .query()
            .table_name("eth-peer-data")
            .key_condition_expression("#id = :id")
            .expression_attribute_names("#id", "peer-id")
            .expression_attribute_values(":id", AttributeValue::S(id))
            .send()
            .await?;

        if let Some(nodes) = results.items {
            let node = nodes.iter().map(|v| v.into()).collect();
            Ok(Some(node))
        } else {
            Ok(None)
        }
    }

    async fn node_by_ip(&self, ip: String) -> Result<Option<Vec<PeerData>>, QueryItemError> {
        let results = self
            .client
            .query()
            .table_name("eth-peer-data")
            .index_name("peer-ip-index")
            .key_condition_expression("#ip = :ip")
            .expression_attribute_names("#ip", "peer-ip")
            .expression_attribute_values(":ip", AttributeValue::S(ip))
            .send()
            .await?;

        if let Some(nodes) = results.items {
            let node = nodes.iter().map(|v| v.into()).collect();
            Ok(Some(node))
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone)]
pub struct InMemoryPeerDB {
    db: Arc<RwLock<HashMap<String, PeerData>>>,
}

impl InMemoryPeerDB {
    pub fn new() -> Self {
        Self {
            db: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl PeerDB for InMemoryPeerDB {
    async fn add_peer(&self, peer_data: PeerData, _: Option<i64>) -> Result<(), AddItemError> {
        let mut db = self
            .db
            .write()
            .map_err(|_| AddItemError::InMemoryDbAddItemError())?;
        db.insert(peer_data.id.clone(), peer_data);
        Ok(())
    }

    async fn all_peers(&self, page_size: Option<i32>) -> Result<Vec<PeerData>, ScanTableError> {
        let page_size = page_size.unwrap_or(50);
        let db = self
            .db
            .read()
            .map_err(|_| ScanTableError::InMemoryDbScanError())?;
        Ok(db
            .iter()
            .map(|(_, peer_data)| peer_data.clone())
            .take(page_size as usize)
            .collect())
    }

    async fn node_by_id(&self, id: String) -> Result<Option<Vec<PeerData>>, QueryItemError> {
        let db = self
            .db
            .read()
            .map_err(|_| QueryItemError::InMemoryDbQueryItemError())?;
        Ok(Some(
            db.iter()
                .filter(|(peer_id, _)| **peer_id == id)
                .map(|(_, peer_data)| peer_data.clone())
                .collect(),
        ))
    }

    async fn node_by_ip(&self, ip: String) -> Result<Option<Vec<PeerData>>, QueryItemError> {
        let db = self
            .db
            .read()
            .map_err(|_| QueryItemError::InMemoryDbQueryItemError())?;
        Ok(Some(
            db.iter()
                .filter(|(_, peer_data)| peer_data.address == ip)
                .map(|(_, peer_data)| peer_data.clone())
                .collect(),
        ))
    }
}

pub struct SqlPeerDB {
    db: Connection,
}

impl SqlPeerDB {
    pub async fn new() -> Self {
        let db = Connection::open("peers_data.db").await.unwrap();
        // create `eth_peer_data` table if not exists
        let _ = db
            .call(|conn| {
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS eth_peer_data (
                id TEXT PRIMARY KEY,
                ip TEXT NOT NULL,
                client_version TEXT NOT NULL,
                enode_url TEXT NOT NULL,
                port INTEGER NOT NULL,
                chain TEXT NOT NULL,
                genesis_hash TEXT NOT NULL,
                best_block TEXT NOT NULL,
                total_difficulty TEXT NOT NULL,
                country TEXT,
                city TEXT,
                last_seen TEXT NOT NULL,
                capabilities TEXT,
                eth_version INTEGER
            );",
                    [],
                )
            })
            .await
            .unwrap();
        Self { db }
    }
}

#[async_trait]
impl PeerDB for SqlPeerDB {
    async fn add_peer(&self, peer_data: PeerData, _: Option<i64>) -> Result<(), AddItemError> {
        let cap = &peer_data.capabilities.join(",");
        self.db
            .call(move |conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO eth_peer_data (id, ip, client_version, enode_url, port, chain, genesis_hash, best_block, total_difficulty, country, city, last_seen, capabilities, eth_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    (
                        &peer_data.id,
                        &peer_data.address,
                        &peer_data.client_version,
                        &peer_data.enode_url,
                        &peer_data.tcp_port,
                        &peer_data.chain,
                        &peer_data.genesis_block_hash,
                        &peer_data.best_block,
                        &peer_data.total_difficulty,
                        &peer_data.country,
                        &peer_data.city,
                        &peer_data.last_seen,
                        &peer_data.capabilities.join(","),
                        &peer_data.eth_version,
                    ),
                )
            })
            .await
            .map_err(|err| AddItemError::SqlAddItemError(err))?;
        Ok(())
    }

    async fn all_peers(&self, page_size: Option<i32>) -> Result<Vec<PeerData>, ScanTableError> {
        let peers = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare("SELECT * from eth_peer_data")?;
                let rows = stmt.query_map([], |row| {
                    Ok(PeerData {
                        id: row.get(0)?,
                        address: row.get(1)?,
                        client_version: row.get(2)?,
                        enode_url: row.get(3)?,
                        tcp_port: row.get(4)?,
                        chain: row.get(5)?,
                        genesis_block_hash: row.get(6)?,
                        best_block: row.get(7)?,
                        total_difficulty: row.get(8)?,
                        country: row.get(9)?,
                        city: row.get(10)?,
                        last_seen: row.get(11)?,
                        capabilities: row
                            .get::<_, String>(12)?
                            .as_str()
                            .split(",")
                            .into_iter()
                            .map(|s| s.to_string())
                            .collect(),
                        eth_version: row.get(13)?,
                    })
                })?;
                let mut peers = vec![];
                for row in rows {
                    if let Ok(peer_data) = row {
                        peers.push(peer_data);
                    }
                }
                Ok(peers)
            })
            .await
            .map_err(|err| ScanTableError::SqlScanError(err))?;

        Ok(peers)
    }

    async fn node_by_id(&self, id: String) -> Result<Option<Vec<PeerData>>, QueryItemError> {
        let peers = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare("SELECT * from eth_peer_data WHERE id = ?1")?;
                let rows = stmt.query_map([id], |row| {
                    Ok(PeerData {
                        id: row.get(0)?,
                        address: row.get(1)?,
                        client_version: row.get(2)?,
                        enode_url: row.get(3)?,
                        tcp_port: row.get(4)?,
                        chain: row.get(5)?,
                        genesis_block_hash: row.get(6)?,
                        best_block: row.get(7)?,
                        total_difficulty: row.get(8)?,
                        country: row.get(9)?,
                        city: row.get(10)?,
                        last_seen: row.get(11)?,
                        capabilities: row
                            .get::<_, String>(12)?
                            .as_str()
                            .split(",")
                            .into_iter()
                            .map(|s| s.to_string())
                            .collect(),
                        eth_version: row.get(13)?,
                    })
                })?;
                let mut peers = vec![];
                for row in rows {
                    if let Ok(peer_data) = row {
                        peers.push(peer_data);
                    }
                }
                Ok(peers)
            })
            .await
            .map_err(|err| QueryItemError::SqlQueryItemError(err))?;

        Ok(Some(peers))
    }

    async fn node_by_ip(&self, ip: String) -> Result<Option<Vec<PeerData>>, QueryItemError> {
        let peers = self
            .db
            .call(move |conn| {
                let mut stmt = conn.prepare("SELECT * from eth_peer_data WHERE ip = ?1")?;
                let rows = stmt.query_map([ip], |row| {
                    Ok(PeerData {
                        id: row.get(0)?,
                        address: row.get(1)?,
                        client_version: row.get(2)?,
                        enode_url: row.get(3)?,
                        tcp_port: row.get(4)?,
                        chain: row.get(5)?,
                        genesis_block_hash: row.get(6)?,
                        best_block: row.get(7)?,
                        total_difficulty: row.get(8)?,
                        country: row.get(9)?,
                        city: row.get(10)?,
                        last_seen: row.get(11)?,
                        capabilities: row
                            .get::<_, String>(12)?
                            .as_str()
                            .split(",")
                            .into_iter()
                            .map(|s| s.to_string())
                            .collect(),
                        eth_version: row.get(13)?,
                    })
                })?;
                let mut peers = vec![];
                for row in rows {
                    if let Ok(peer_data) = row {
                        peers.push(peer_data);
                    }
                }
                Ok(peers)
            })
            .await
            .map_err(|err| QueryItemError::SqlQueryItemError(err))?;

        Ok(Some(peers))
    }
}

impl SqlPeerDB {
    /// Prune peers that are older than `time_validity`. Note that `time_validity` **MUST** be in days.
    pub async fn prune_peers(&self, time_validity: i64) -> Result<(), DeleteItemError> {
        let cutoff = Utc::now()
            .checked_sub_signed(Duration::days(time_validity))
            .unwrap()
            .to_string();
        let deleted_peers_number = self
            .db
            .call(move |conn| {
                conn.execute(
                    "DELETE FROM eth_peer_data WHERE last_seen < ?1 ",
                    [cutoff.as_str()],
                )
            })
            .await
            .map_err(|err| DeleteItemError::SqlDeleteItemError(err))?;

        info!("Number of peers pruned: {}", deleted_peers_number);
        Ok(())
    }
}
