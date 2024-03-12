#!/bin/bash

# Check if the correct number of arguments was provided
if [ "$#" -lt 4 ]; then
    echo "Usage: $0 INITIAL_BLOCK NUM_BLOCKS RPC_ENDPOINT RPC_TYPE BACKOFF RETRIES"
    exit 1
fi

# Read the arguments
# 1 --> initial block number
# 2 --> number of blocks to debug
# 3 --> Rpc endpoint:port (eg. http://35.246.1.96:8545)
# 4 --> Rpc type (eg. jerigon / native)
# 5 --> Backoff time (in milliseconds) for the RPC requests
# 6 --> Number of retries for the RPC requests
INITIAL_BLOCK=$1
NUM_BLOCKS=$2
RPC_ENDPOINT=$3
RPC_TYPE=$4
BACKOFF=${5:-0}
RETRIES=${6:-0}

# Get the directory of the current script
script_dir=$(dirname "$0")

# Loop and call the existing script
for (( i=0; i<NUM_BLOCKS; i++ )); do
    BLOCK=$((INITIAL_BLOCK + i))
    echo "Running debug block script with block=$BLOCK rpc=$RPC_ENDPOINT rpc type=$RPC_TYPE backoff=$BACKOFF retries=$RETRIES"
    "$script_dir/debug_block.sh" $BLOCK $RPC_ENDPOINT $RPC_TYPE $BACKOFF $RETRIES
done
