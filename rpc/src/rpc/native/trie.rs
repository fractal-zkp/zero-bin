use std::collections::HashMap;
use std::sync::Arc;

use ethereum_types::H256;
use ethers::{
    types::Bytes,
    utils::{keccak256, rlp},
};
use mpt_trie::{
    nibbles::Nibbles,
    partial_trie::{HashedPartialTrie, Node, PartialTrie, WrappedNode},
};

/// A builder for constructing a partial trie from a collection of nodes.
pub struct HashedPartialTrieBuilder {
    root: H256,
    nodes: HashMap<H256, Vec<u8>>,
}

impl HashedPartialTrieBuilder {
    /// Creates a new `HashedPartialTrieBuilder` with the given root and nodes.
    pub fn new(root: H256, nodes: HashMap<H256, Vec<u8>>) -> Self {
        HashedPartialTrieBuilder { root, nodes }
    }

    /// Inserts a proof into the builder.
    ///
    /// The proof is a collection of nodes that are used to construct the
    /// partial trie.
    pub fn insert_proof(&mut self, proof: Vec<Bytes>) {
        for node in proof {
            self.nodes.insert(keccak256(&node).into(), node.to_vec());
        }
    }

    /// Builds the partial trie from the nodes and root.
    pub fn build(self) -> HashedPartialTrie {
        construct_partial_trie(self.root, &self.nodes)
    }
}

/// Constructs a partial trie from a root hash and a collection of nodes.
fn construct_partial_trie<T: PartialTrie>(hash: H256, nodes: &HashMap<H256, Vec<u8>>) -> T {
    let bytes = match nodes.get(&hash) {
        Some(value) => rlp::decode_list::<Vec<u8>>(value),
        None => return T::new(Node::Hash(hash)),
    };

    let node = match bytes.len() {
        17 => parse_branch_node(bytes, nodes),
        2 if is_extension_node(&bytes) => parse_extension_node(bytes, nodes),
        2 if is_leaf_node(&bytes) => parse_leaf_node(bytes),
        _ => unreachable!(),
    };

    T::new(node)
}

/// Returns true if the node is an extension node.
fn is_extension_node(bytes: &[Vec<u8>]) -> bool {
    (bytes[0][0] >> 4 == 0) | (bytes[0][0] >> 4 == 1)
}

/// Returns true if the node is a leaf node.
fn is_leaf_node(bytes: &[Vec<u8>]) -> bool {
    (bytes[0][0] >> 4 == 2) | (bytes[0][0] >> 4 == 3)
}

/// Parses a branch node from the given bytes.
fn parse_branch_node<T: PartialTrie>(
    bytes: Vec<Vec<u8>>,
    nodes: &HashMap<H256, Vec<u8>>,
) -> Node<T> {
    let children = (0..16)
        .map(|i| {
            let child = match bytes[i].is_empty() {
                true => T::default(),
                false => construct_partial_trie(H256::from_slice(&bytes[i]), nodes),
            };
            Arc::new(Box::new(child))
        })
        .collect::<Vec<WrappedNode<T>>>();

    Node::<T>::Branch {
        children: children.try_into().unwrap(),
        value: bytes[16].clone(),
    }
}

/// Parses an extension node from the given bytes.
fn parse_extension_node<T: PartialTrie>(
    bytes: Vec<Vec<u8>>,
    nodes: &HashMap<H256, Vec<u8>>,
) -> Node<T> {
    let mut encoded_path = Nibbles::from_bytes_be(&bytes[0][..]).unwrap();

    if encoded_path.pop_nibbles_front(1).get_nibble(0) == 0 {
        encoded_path.pop_nibbles_front(1);
    }

    Node::Extension {
        nibbles: encoded_path,
        child: Arc::new(Box::new(construct_partial_trie(
            H256::from_slice(&bytes[1]),
            nodes,
        ))),
    }
}

/// Parses a leaf node from the given bytes.
fn parse_leaf_node<T: PartialTrie>(bytes: Vec<Vec<u8>>) -> Node<T> {
    let mut encoded_path = Nibbles::from_bytes_be(&bytes[0][..]).unwrap();

    if encoded_path.pop_nibbles_front(1).get_nibble(0) == 2 {
        encoded_path.pop_nibbles_front(1);
    }

    Node::Leaf {
        nibbles: encoded_path,
        value: bytes[1].clone(),
    }
}
