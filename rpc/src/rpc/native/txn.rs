use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use ethers::providers::Middleware;
use ethers::providers::{Http, Provider};
use ethers::types::AccountState;
use ethers::types::{
    transaction::eip2930::AccessList, Address, DiffMode, GethTrace, GethTraceFrame, PreStateFrame,
    PreStateMode, TransactionReceipt, H160, H256, U256,
};
use ethers::utils::keccak256;
use ethers::utils::rlp;
use tokio::sync::Mutex;
use trace_decoder::trace_protocol::ContractCodeUsage;
use trace_decoder::trace_protocol::TxnTrace;
use trace_decoder::trace_protocol::{TxnInfo, TxnMeta};

use super::tracing_options;
use super::tracing_options_diff;

/// Processes the transaction with the given transaction hash and updates the
/// accounts state.
pub(super) async fn process_transaction(
    provider: Arc<Provider<Http>>,
    tx_hash: &H256,
    accounts_state: Arc<Mutex<HashMap<H160, HashSet<H256>>>>,
    code_db: Arc<Mutex<HashMap<H256, Vec<u8>>>>,
) -> Result<TxnInfo> {
    let (tx, tx_receipt, pre_trace, diff_trace) = fetch_tx_data(provider, tx_hash).await?;
    let tx_meta = TxnMeta {
        byte_code: tx.rlp().to_vec(),
        new_txn_trie_node_byte: tx.rlp().to_vec(),
        new_receipt_trie_node_byte: compute_receipt_bytes(&tx_receipt),
        gas_used: tx_receipt.gas_used.unwrap().as_u64(),
    };

    let access_list = parse_access_list(tx.access_list.unwrap_or_default());
    let mut accounts_state = accounts_state.lock().await;
    let mut code_db = code_db.lock().await;
    let tx_traces = match (pre_trace, diff_trace) {
        (
            GethTrace::Known(GethTraceFrame::PreStateTracer(PreStateFrame::Default(read))),
            GethTrace::Known(GethTraceFrame::PreStateTracer(PreStateFrame::Diff(diff))),
        ) => process_tx_traces(&mut accounts_state, &mut code_db, access_list, read, diff)?,
        _ => unreachable!(),
    };

    Ok(TxnInfo {
        meta: tx_meta,
        traces: tx_traces,
    })
}

/// Fetches the transaction data for the given transaction hash.
async fn fetch_tx_data(
    provider: Arc<Provider<Http>>,
    tx_hash: &H256,
) -> Result<
    (
        ethers::types::Transaction,
        TransactionReceipt,
        GethTrace,
        GethTrace,
    ),
    anyhow::Error,
> {
    let tx_fut = provider.get_transaction(*tx_hash);
    let tx_receipt_fut = provider.get_transaction_receipt(*tx_hash);
    let pre_trace_fut = provider.debug_trace_transaction(*tx_hash, tracing_options());
    let diff_trace_fut = provider.debug_trace_transaction(*tx_hash, tracing_options_diff());

    let (tx, tx_receipt, pre_trace, diff_trace) =
        futures::try_join!(tx_fut, tx_receipt_fut, pre_trace_fut, diff_trace_fut,)?;

    Ok((
        tx.ok_or_else(|| anyhow!("Transaction not found."))?,
        tx_receipt.ok_or_else(|| anyhow!("Transaction receipt not found."))?,
        pre_trace,
        diff_trace,
    ))
}

// TODO: upstream this to ethers
/// Computes the receipt bytes for the given transaction receipt.
///
/// NOTE: The ethers-rs library does not encode the transaction type in the
/// transaction receipt hence we do it manually here.
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

/// Parse the access list data into a hashmap.
fn parse_access_list(access_list: AccessList) -> HashMap<H160, HashSet<H256>> {
    let mut result = HashMap::new();

    for item in access_list.0.into_iter() {
        result
            .entry(item.address)
            .or_insert_with(HashSet::new)
            .extend(item.storage_keys);
    }

    result
}

/// Processes the transaction traces and updates the accounts state.
fn process_tx_traces(
    accounts_state: &mut HashMap<H160, HashSet<H256>>,
    code_db: &mut HashMap<H256, Vec<u8>>,
    mut access_list: HashMap<H160, HashSet<H256>>,
    read_trace: PreStateMode,
    diff_trace: DiffMode,
) -> Result<HashMap<Address, TxnTrace>> {
    let DiffMode {
        pre: pre_trace,
        post: post_trace,
    } = diff_trace;

    let addresses: HashSet<_> = read_trace
        .0
        .keys()
        .chain(post_trace.keys())
        .chain(pre_trace.keys())
        .chain(access_list.keys())
        .copied()
        .collect();

    Ok(addresses
        .into_iter()
        .map(|address| {
            let storage_keys = accounts_state.entry(address).or_insert(Default::default());
            let read_state = read_trace.0.get(&address);
            let pre_state = pre_trace.get(&address);
            let post_state = post_trace.get(&address);

            let balance = post_state.and_then(|x| x.balance);
            let (storage_read, storage_written) = process_storage(
                storage_keys,
                access_list.remove(&address).unwrap_or_default(),
                read_state,
                post_state,
                pre_state,
            );
            let code_usage = process_code(post_state, read_state, code_db);
            let nonce = post_state.and_then(|x| x.nonce).or_else(|| {
                if let Some(ContractCodeUsage::Write(_)) = code_usage.as_ref() {
                    Some(U256::from(1))
                } else {
                    None
                }
            });
            let self_destructed = process_self_destruct(post_state, pre_state);

            (
                address,
                TxnTrace {
                    balance,
                    nonce,
                    storage_read,
                    storage_written,
                    code_usage,
                    self_destructed,
                },
            )
        })
        .collect::<HashMap<H160, TxnTrace>>())
}

/// Processes the storage for the given account state.
///
/// Returns the storage read and written for the given account in the
/// transaction and updates the storage keys.
fn process_storage(
    storage_keys: &mut HashSet<H256>,
    access_list: HashSet<H256>,
    acct_state: Option<&AccountState>,
    post_acct: Option<&AccountState>,
    pre_acct: Option<&AccountState>,
) -> (Option<Vec<H256>>, Option<HashMap<H256, U256>>) {
    let mut storage_read = access_list;
    storage_read.extend(
        acct_state
            .and_then(|acct| {
                acct.storage
                    .as_ref()
                    .map(|x| x.keys().copied().collect::<Vec<H256>>())
            })
            .unwrap_or_default(),
    );

    let mut storage_written: HashMap<H256, U256> = post_acct
        .and_then(|x| {
            x.storage.as_ref().map(|s| {
                s.iter()
                    .map(|(k, v)| (*k, U256::from_big_endian(&v.0)))
                    .collect()
            })
        })
        .unwrap_or_default();

    // Add the deleted keys to the storage written
    pre_acct.and_then(|x| {
        x.storage.as_ref().map(|s| {
            for key in s.keys() {
                storage_written.entry(*key).or_insert(U256::zero());
            }
        })
    });

    storage_keys.extend(storage_written.keys().copied());
    storage_keys.extend(storage_read.iter().copied());

    (
        Option::from(storage_read.into_iter().collect::<Vec<H256>>()).filter(|v| !v.is_empty()),
        Option::from(storage_written).filter(|v| !v.is_empty()),
    )
}

/// Processes the code usage for the given account state.
fn process_code(
    post_state: Option<&AccountState>,
    read_state: Option<&AccountState>,
    code_db: &mut HashMap<H256, Vec<u8>>,
) -> Option<ContractCodeUsage> {
    let code_usage = post_state.and_then(|x| x.code.as_ref()).map_or(
        read_state.and_then(|acct| {
            acct.code.as_ref().map(|x| {
                let code = hex::decode(&x[2..]).expect("must be valid");
                let code_hash = keccak256(&code).into();
                code_db.insert(code_hash, code);
                ContractCodeUsage::Read(code_hash)
            })
        }),
        |x| {
            let code = hex::decode(&x[2..]).expect("must be valid");
            let code_hash = keccak256(&code).into();
            code_db.insert(code_hash, code.clone());
            Some(ContractCodeUsage::Write(code.into()))
        },
    );
    code_usage
}

/// Processes the self destruct for the given account state.
fn process_self_destruct(
    post_state: Option<&AccountState>,
    pre_state: Option<&AccountState>,
) -> Option<bool> {
    let self_destructed = if post_state.is_none() && pre_state.is_some() {
        Some(true)
    } else {
        None
    };
    self_destructed
}
