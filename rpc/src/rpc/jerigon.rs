use anyhow::{Context, Result};
use async_trait::async_trait;
use ethereum_types::{Address, Bloom, H256, U256};
use evm_arithmetization::proof::{BlockHashes, BlockMetadata};
use futures::{stream::FuturesOrdered, TryStreamExt};
use reqwest::Client;
use reqwest::IntoUrl;
use serde::Deserialize;
use thiserror::Error;
use tokio::try_join;
use trace_decoder::{
    trace_protocol::{BlockTrace, BlockTraceTriePreImages, TxnInfo},
    types::{BlockLevelData, OtherBlockData},
};
use tracing::{debug, info};

use super::{ProverInput, RpcClient};

pub struct JerigonRpcClient {
    client: Client,
    rpc_url: String,
}

impl JerigonRpcClient {
    pub fn new(rpc_url: String) -> Self {
        Self {
            client: Client::new(),
            rpc_url,
        }
    }
}

#[async_trait]
impl RpcClient for JerigonRpcClient {
    async fn fetch_prover_input(
        &self,
        block_number: u64,
        checkpoint_block_number: u64,
    ) -> Result<ProverInput> {
        let (trace_result, rpc_block_metadata) = try_join!(
            JerigonTraceResponse::fetch(&self.client, &self.rpc_url, block_number),
            RpcBlockMetadata::fetch(
                &self.client,
                &self.rpc_url,
                block_number,
                checkpoint_block_number
            ),
        )?;

        debug!("Got block result: {:?}", rpc_block_metadata.block_by_number);
        debug!("Got trace result: {:?}", trace_result);
        debug!("Got chain_id: {:?}", rpc_block_metadata.chain_id);

        Ok(ProverInput {
            block_trace: trace_result.try_into()?,
            other_data: rpc_block_metadata.into(),
        })
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub(crate) enum JerigonResultItem {
    Result(TxnInfo),
    BlockWitness(BlockTraceTriePreImages),
}

/// The response from the `debug_traceBlockByNumber` RPC method.
#[derive(Deserialize, Debug)]
pub(crate) struct JerigonTraceResponse {
    pub(crate) result: Vec<JerigonResultItem>,
}

#[derive(Error, Debug)]
pub(crate) enum JerigonTraceError {
    #[error("expected BlockTraceTriePreImages in block_witness key")]
    BlockTraceTriePreImagesNotFound,
}

impl TryFrom<JerigonTraceResponse> for BlockTrace {
    type Error = JerigonTraceError;

    fn try_from(value: JerigonTraceResponse) -> Result<Self, Self::Error> {
        let mut txn_info = Vec::new();
        let mut trie_pre_images = None;

        for item in value.result {
            match item {
                JerigonResultItem::Result(info) => {
                    txn_info.push(info);
                }
                JerigonResultItem::BlockWitness(pre_images) => {
                    trie_pre_images = Some(pre_images);
                }
            }
        }

        let trie_pre_images =
            trie_pre_images.ok_or(JerigonTraceError::BlockTraceTriePreImagesNotFound)?;

        Ok(Self {
            txn_info,
            trie_pre_images,
        })
    }
}

impl JerigonTraceResponse {
    /// Fetches the block trace for the given block number.
    pub(crate) async fn fetch<U: IntoUrl>(
        client: &reqwest::Client,
        rpc_url: U,
        block_number: u64,
    ) -> Result<Self> {
        let block_number_hex = format!("0x{:x}", block_number);
        info!("Fetching block trace for block {}", block_number_hex);

        let response = client
            .post(rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "debug_traceBlockByNumber",
                "params": [&block_number_hex, {"tracer": "zeroTracer"}],
                "id": 1,
            }))
            .send()
            .await
            .context("fetching debug_traceBlockByNumber")?;

        let bytes = response.bytes().await?;
        let des = &mut serde_json::Deserializer::from_slice(&bytes);
        let parsed = serde_path_to_error::deserialize(des)
            .context("deserializing debug_traceBlockByNumber")?;

        Ok(parsed)
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EthGetBlockByNumberResult {
    pub(crate) base_fee_per_gas: U256,
    pub(crate) difficulty: U256,
    pub(crate) gas_limit: U256,
    pub(crate) gas_used: U256,
    pub(crate) hash: H256,
    pub(crate) logs_bloom: Bloom,
    pub(crate) miner: Address,
    pub(crate) mix_hash: H256,
    pub(crate) number: U256,
    pub(crate) parent_hash: H256,
    pub(crate) state_root: H256,
    pub(crate) timestamp: U256,
    pub(crate) withdrawals: Vec<Withdrawal>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Withdrawal {
    pub(crate) address: Address,
    pub(crate) amount: U256,
}

impl From<Withdrawal> for (Address, U256) {
    fn from(v: Withdrawal) -> Self {
        (v.address, v.amount)
    }
}

/// The response from the `eth_getBlockByNumber` RPC method.
#[derive(Deserialize, Debug)]
pub(crate) struct EthGetBlockByNumberResponse {
    pub(crate) result: EthGetBlockByNumberResult,
}

impl EthGetBlockByNumberResponse {
    /// Fetches the block metadata for the given block number.
    pub(crate) async fn fetch<U: IntoUrl>(
        client: &Client,
        rpc_url: U,
        block_number: u64,
    ) -> Result<Self> {
        let block_number_hex = format!("0x{:x}", block_number);
        info!("Fetching block metadata for block {}", block_number_hex);

        let response = client
            .post(rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_getBlockByNumber",
                "params": [&block_number_hex, false],
                "id": 1,
            }))
            .send()
            .await
            .context("fetching eth_getBlockByNumber")?;

        let bytes = response.bytes().await?;
        let des = &mut serde_json::Deserializer::from_slice(&bytes);
        let parsed =
            serde_path_to_error::deserialize(des).context("deserializing eth_getBlockByNumber")?;

        Ok(parsed)
    }

    pub(crate) async fn fetch_previous_block_hashes<U: IntoUrl + Copy>(
        client: &Client,
        rpc_url: U,
        block_number: u64,
    ) -> Result<Vec<H256>> {
        if block_number == 0 {
            return Ok(vec![H256::default(); 256]);
        }

        let mut hashes = Vec::with_capacity(256);

        let padding_delta = block_number as i64 - 256;
        if padding_delta < 0 {
            let default = H256::default();
            for _ in 0..padding_delta.abs() {
                hashes.push(default);
            }
        }

        // Every block response includes the _parent_ hash along with its hash, so we
        // can just fetch half the blocks to acquire all hashes for the range.
        let start = block_number.saturating_sub(256);
        let mut futs: FuturesOrdered<_> = (start..=block_number)
            .step_by(2)
            .map(|block_number| Self::fetch(client, rpc_url, block_number))
            .collect();

        while let Some(response) = futs.try_next().await? {
            // Ignore hash of the current block.
            if response.result.number == block_number.into() {
                hashes.push(response.result.parent_hash);
                continue;
            }

            // Ignore the parent of the start block.
            if response.result.number != start.into() {
                hashes.push(response.result.parent_hash);
            }

            hashes.push(response.result.hash);
        }

        Ok(hashes)
    }

    pub(crate) async fn fetch_checkpoint_state_trie_root<U: IntoUrl + Copy>(
        client: &Client,
        rpc_url: U,
        block_number: u64,
    ) -> Result<H256> {
        let res = Self::fetch(client, rpc_url, block_number).await?;
        Ok(res.result.state_root)
    }
}

/// The response from the `eth_chainId` RPC method.
#[derive(Deserialize, Debug)]
pub(crate) struct EthChainIdResponse {
    pub(crate) result: U256,
}

impl EthChainIdResponse {
    /// Fetches the chain id.
    pub(crate) async fn fetch<U: IntoUrl>(client: &Client, rpc_url: U) -> Result<Self> {
        info!("Fetching chain id");

        let response = client
            .post(rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_chainId",
                "params": [],
                "id": 1,
            }))
            .send()
            .await
            .context("fetching eth_chainId")?;

        let bytes = response.bytes().await?;
        let des = &mut serde_json::Deserializer::from_slice(&bytes);
        let parsed = serde_path_to_error::deserialize(des).context("deserializing eth_chainId")?;

        Ok(parsed)
    }
}

/// Product of the `eth_getBlockByNumber` and `eth_chainId` RPC methods.
///
/// Contains the necessary data to construct the `OtherBlockData` struct.
pub(crate) struct RpcBlockMetadata {
    pub(crate) block_by_number: EthGetBlockByNumberResponse,
    pub(crate) chain_id: EthChainIdResponse,
    pub(crate) prev_hashes: Vec<H256>,
    pub(crate) checkpoint_state_trie_root: H256,
}

impl RpcBlockMetadata {
    pub(crate) async fn fetch(
        client: &Client,
        rpc_url: &str,
        block_number: u64,
        checkpoint_block_number: u64,
    ) -> Result<Self> {
        let (block_result, chain_id_result, prev_hashes, checkpoint_state_trie_root) = try_join!(
            EthGetBlockByNumberResponse::fetch(client, rpc_url, block_number),
            EthChainIdResponse::fetch(client, rpc_url),
            EthGetBlockByNumberResponse::fetch_previous_block_hashes(client, rpc_url, block_number),
            EthGetBlockByNumberResponse::fetch_checkpoint_state_trie_root(
                client,
                rpc_url,
                checkpoint_block_number
            )
        )?;

        Ok(Self {
            block_by_number: block_result,
            chain_id: chain_id_result,
            prev_hashes,
            checkpoint_state_trie_root,
        })
    }
}

impl From<RpcBlockMetadata> for OtherBlockData {
    fn from(
        RpcBlockMetadata {
            block_by_number,
            chain_id,
            prev_hashes,
            checkpoint_state_trie_root,
        }: RpcBlockMetadata,
    ) -> Self {
        let mut bloom = [U256::zero(); 8];

        for (i, word) in block_by_number
            .result
            .logs_bloom
            .as_fixed_bytes()
            .chunks_exact(32)
            .enumerate()
        {
            bloom[i] = U256::from_big_endian(word);
        }

        let block_metadata = BlockMetadata {
            block_beneficiary: block_by_number.result.miner,
            block_timestamp: block_by_number.result.timestamp,
            block_number: block_by_number.result.number,
            block_difficulty: block_by_number.result.difficulty,
            block_random: block_by_number.result.mix_hash,
            block_gaslimit: block_by_number.result.gas_limit,
            block_chain_id: chain_id.result,
            block_base_fee: block_by_number.result.base_fee_per_gas,
            block_gas_used: block_by_number.result.gas_used,
            block_bloom: bloom,
        };

        let withdrawals = block_by_number
            .result
            .withdrawals
            .into_iter()
            .map(|w| w.into())
            .collect();
        Self {
            b_data: BlockLevelData {
                b_meta: block_metadata,
                b_hashes: BlockHashes {
                    prev_hashes,
                    cur_hash: block_by_number.result.hash,
                },
                withdrawals,
            },
            checkpoint_state_trie_root,
        }
    }
}
