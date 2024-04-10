#![allow(clippy::needless_range_loop)]
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use ethers::prelude::*;
use ethers::types::{GethDebugTracerType, H256, U256};
use ethers::utils::keccak256;
use ethers::utils::rlp;
use futures::stream::{self, TryStreamExt};
use reqwest::Client;
use trace_decoder::trace_protocol::TrieDirect;
use trace_decoder::trace_protocol::{
    BlockTrace, BlockTraceTriePreImages, ContractCodeUsage, SeparateStorageTriesPreImage,
    SeparateTriePreImage, SeparateTriePreImages, TxnInfo, TxnMeta, TxnTrace,
};
use trace_decoder::types::HashedStorageAddr;

use super::{async_trait, jerigon::RpcBlockMetadata, ProverInput, RpcClient};

mod trie;
use trie::HashedPartialTrieBuilder;

/// The native RPC client.
pub struct NativeRpcClient {
    provider: Provider<Http>,
    rpc_url: String,
}

impl NativeRpcClient {
    /// Creates a new `NativeRpcClient` with the given RPC URL.
    pub fn new(rpc_url: String) -> Result<Self> {
        let provider = Provider::<Http>::try_from(rpc_url.clone())?;
        Ok(Self { provider, rpc_url })
    }
}

#[async_trait]
impl RpcClient for NativeRpcClient {
    async fn fetch_prover_input(
        &self,
        block_number: u64,
        checkpoint_block_number: u64,
    ) -> Result<ProverInput> {
        let accounts_state = Arc::new(Mutex::new(BTreeMap::<H160, AccountState>::new()));
        let block = self
            .provider
            .get_block(block_number)
            .await?
            .ok_or_else(|| anyhow!("Block not found. Block number: {}", block_number))?;

        let tx_infos = stream::iter(&block.transactions)
            .then(|tx_hash| {
                let arc = accounts_state.clone();
                async move { process_transaction(&self.provider, tx_hash, arc).await }
            })
            .try_collect::<Vec<TxnInfo>>()
            .await?;

        let block_trace = BlockTrace {
            txn_info: tx_infos,
            trie_pre_images: process_block_trace_witness(&self.provider, block, accounts_state)
                .await?,
        };

        let client = Client::new();
        Ok(ProverInput {
            block_trace,
            other_data: RpcBlockMetadata::fetch(
                &client,
                &self.rpc_url,
                block_number,
                checkpoint_block_number,
            )
            .await?
            .into(),
        })
    }
}

async fn process_block_trace_witness(
    provider: &Provider<Http>,
    block: Block<H256>,
    accounts_state: Arc<Mutex<BTreeMap<H160, AccountState>>>,
) -> Result<BlockTraceTriePreImages> {
    let accounts_state = Arc::try_unwrap(accounts_state)
        .unwrap()
        .into_inner()
        .unwrap();

    let block_number = block.number.unwrap();
    let prev_block = provider
        .get_block(block_number - 1)
        .await?
        .ok_or_else(|| anyhow!("Block not found. Block number: {}", block_number - 1))?;

    let mut state = HashedPartialTrieBuilder::new(prev_block.state_root, Default::default());
    let mut storage_proofs = HashMap::<HashedStorageAddr, HashedPartialTrieBuilder>::new();
    for (address, account) in accounts_state.iter() {
        let proof = provider
            .get_proof(
                *address,
                account
                    .storage
                    .as_ref()
                    .map_or(vec![], |x| x.keys().copied().collect()),
                Some((block_number - 1).into()),
            )
            .await?;
        state.insert_proof(proof.account_proof);

        if account.storage.is_some() {
            let mut storage_mpt =
                HashedPartialTrieBuilder::new(proof.storage_hash, Default::default());
            for proof in proof.storage_proof {
                storage_mpt.insert_proof(proof.proof);
            }

            storage_proofs.insert(keccak256(address).into(), storage_mpt);
        }
    }

    let proof = provider
        .get_proof(
            block.author.expect("block must have author"),
            vec![],
            block.number.map(Into::into),
        )
        .await?;
    state.insert_proof(proof.account_proof);

    if let Some(withdrawals) = block.withdrawals.as_ref() {
        for withdrawal in withdrawals {
            let proof = provider
                .get_proof(withdrawal.address, vec![], Some((block_number - 1).into()))
                .await?;
            state.insert_proof(proof.account_proof);
        }
    }

    Ok(BlockTraceTriePreImages::Separate(SeparateTriePreImages {
        state: SeparateTriePreImage::Direct(TrieDirect(state.build())),
        storage: SeparateStorageTriesPreImage::MultipleTries(
            storage_proofs
                .into_iter()
                .map(|(a, m)| (a, SeparateTriePreImage::Direct(TrieDirect(m.build()))))
                .collect(),
        ),
    }))
}

async fn process_transaction(
    provider: &Provider<Http>,
    tx_hash: &H256,
    accounts_state: Arc<Mutex<BTreeMap<H160, AccountState>>>,
) -> Result<TxnInfo> {
    let tx = provider
        .get_transaction(*tx_hash)
        .await?
        .ok_or_else(|| anyhow!("Transaction not found."))?;
    let tx_receipt = provider
        .get_transaction_receipt(*tx_hash)
        .await?
        .ok_or_else(|| anyhow!("Transaction receipt not found."))?;
    let pre_trace = provider
        .debug_trace_transaction(*tx_hash, tracing_options())
        .await?;
    let diff_trace = provider
        .debug_trace_transaction(*tx_hash, tracing_options_diff())
        .await?;
    let tx_meta = TxnMeta {
        byte_code: tx.rlp().to_vec(),
        new_txn_trie_node_byte: tx.rlp().to_vec(),
        new_receipt_trie_node_byte: compute_receipt_bytes(&tx_receipt),
        gas_used: tx.gas.try_into().expect("gas used must be valid u64"),
    };

    let tx_traces = match (pre_trace, diff_trace) {
        (
            GethTrace::Known(GethTraceFrame::PreStateTracer(PreStateFrame::Default(read))),
            GethTrace::Known(GethTraceFrame::PreStateTracer(PreStateFrame::Diff(diff))),
        ) => process_tx_traces(accounts_state, read, diff).await?,
        _ => unreachable!(),
    };

    Ok(TxnInfo {
        meta: tx_meta,
        traces: tx_traces,
    })
}

fn compute_receipt_bytes(tx_receipt: &TransactionReceipt) -> Vec<u8> {
    let mut bytes = rlp::encode(tx_receipt).to_vec();
    match tx_receipt.transaction_type {
        Some(tx_type) if !tx_type.is_zero() => {
            bytes.insert(0, tx_type.0[0] as u8);
        }
        _ => return bytes,
    }

    rlp::encode(&bytes).to_vec()
}

async fn process_tx_traces(
    accounts_state: Arc<Mutex<BTreeMap<H160, AccountState>>>,
    read_trace: PreStateMode,
    diff_trace: DiffMode,
) -> Result<HashMap<Address, TxnTrace>> {
    let mut accounts_state = accounts_state.lock().unwrap();
    for (address, acct_state) in read_trace.0.iter() {
        accounts_state
            .entry(*address)
            .and_modify(|acct| {
                merge_account_storage(&mut acct.storage, acct_state.storage.as_ref())
            })
            .or_insert(acct_state.clone());
    }

    let DiffMode {
        pre: pre_trace,
        post: post_trace,
    } = diff_trace;

    Ok(read_trace
        .0
        .into_iter()
        .map(|(address, acct_state)| {
            (
                address,
                TxnTrace {
                    balance: post_trace.get(&address).and_then(|x| x.balance),
                    nonce: post_trace.get(&address).and_then(|x| x.nonce),
                    storage_read: acct_state.storage.map(|x| x.keys().copied().collect()),
                    //TODO: check if endianess is correct
                    storage_written: post_trace.get(&address).and_then(|x| {
                        x.storage.as_ref().map(|s| {
                            s.into_iter()
                                .map(|(k, v)| (*k, U256::from_big_endian(v.as_bytes())))
                                .collect()
                        })
                    }),
                    code_usage: post_trace.get(&address).map_or(
                        acct_state.code.map(|x| {
                            ContractCodeUsage::Read(H256::from_str(&x).expect("must be valid"))
                        }),
                        |x| {
                            x.code
                                .as_ref()
                                .map(|x| ContractCodeUsage::Write(x.as_bytes().to_vec().into()))
                        },
                    ),
                    self_destructed: if post_trace.get(&address).is_none()
                        && pre_trace.contains_key(&address)
                    {
                        Some(true)
                    } else {
                        None
                    },
                },
            )
        })
        .collect::<HashMap<H160, TxnTrace>>())
}

fn merge_account_storage(
    storage: &mut Option<BTreeMap<H256, H256>>,
    new_storage: Option<&BTreeMap<H256, H256>>,
) {
    match (storage, new_storage) {
        (Some(storage), Some(new_storage)) => storage.extend(new_storage),
        (storage, Some(new_storage)) => {
            *storage = Some(new_storage.clone());
        }
        _ => (),
    }
}

/// Tracing options for the debug_traceTransaction call.
fn tracing_options() -> GethDebugTracingOptions {
    GethDebugTracingOptions {
        tracer: Some(GethDebugTracerType::BuiltInTracer(
            GethDebugBuiltInTracerType::PreStateTracer,
        )),

        ..GethDebugTracingOptions::default()
    }
}

fn tracing_options_diff() -> GethDebugTracingOptions {
    GethDebugTracingOptions {
        tracer: Some(GethDebugTracerType::BuiltInTracer(
            GethDebugBuiltInTracerType::PreStateTracer,
        )),

        tracer_config: Some(GethDebugTracerConfig::BuiltInTracer(
            GethDebugBuiltInTracerConfig::PreStateTracer(PreStateConfig {
                diff_mode: Some(true),
            }),
        )),
        ..GethDebugTracingOptions::default()
    }
}
