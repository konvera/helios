use std::str::FromStr;

use async_trait::async_trait;

use common::types::{Block, Transactions};
use ethers::types::Block as EthersBlock;
use ethers::prelude::Http;
use ethers::providers::{HttpRateLimitRetryPolicy, Middleware, Provider, RetryClient};

use ethers::types::H256;
use eyre::Result;

use common::errors::RpcError;
use hex::ToHex;

use super::ExecutionRpc;


pub struct HttpRpc {
    url: String,
    provider: Provider<RetryClient<Http>>,
}

impl Clone for HttpRpc {
    fn clone(&self) -> Self {
        Self::new(&self.url).unwrap()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl ExecutionRpc for HttpRpc {
    fn new(rpc: &str) -> Result<Self> {
        let http = Http::from_str(rpc)?;
        let mut client = RetryClient::new(http, Box::new(HttpRateLimitRetryPolicy), 100, 50);
        client.set_compute_units(300);

        let provider = Provider::new(client);

        Ok(HttpRpc {
            url: rpc.to_string(),
            provider,
        })
    }

    async fn get_block_number(
        &self,
    ) -> Result<u64> {
        let number = self
            .provider
            .get_block_number()
            .await
            .map_err(|e| RpcError::new("get_block_number", e))?.as_u64();

        Ok(number)
    }

    async fn get_block_by_number(
        &self,
        number: u64,
    ) -> Result<Block> {
        let block = self
            .provider
            .get_block(number)
            .await
            .map_err(|e| RpcError::new("get_block_by_number", e))?.unwrap();

        Ok(convert_ethers_to_common_block(block).unwrap())
    }
    
    async fn get_block_by_hash(&self, hash: H256) -> Result<Block> {
        let block = self
            .provider
            .get_block(hash)
            .await
            .map_err(|e| RpcError::new("get_block_by_hash", e))?.unwrap();

        Ok(convert_ethers_to_common_block(block).unwrap())
    }

    async fn get_latest_block(
        &self,
    ) -> Result<Block> {
        Ok(self.get_block_by_number(self.get_block_number().await?).await?)
    }
}

fn convert_ethers_to_common_block(block: EthersBlock<H256>) -> Option<Block> {
    Some(Block {
        hash: block.hash?,
        parent_hash: block.parent_hash,
        number: block.number?,
        timestamp: block.timestamp.as_u64().into(),
        transactions: Transactions::Hashes(block.transactions),
        transactions_root: block.transactions_root,
        state_root: block.state_root,
        receipts_root: block.receipts_root,
        gas_used: block.gas_used.as_u64().into(),
        gas_limit: block.gas_limit.as_u64().into(),
        difficulty: block.difficulty,
        total_difficulty: block.total_difficulty?.as_u64().into(),
        size: block.size?.as_u64().into(),
        extra_data: block.extra_data,
        uncles: block.uncles,
        logs_bloom: block.logs_bloom?.data().into(),
        miner: block.author?,
        mix_hash: block.mix_hash?,
        nonce: block.nonce?.encode_hex(),
        base_fee_per_gas: block.base_fee_per_gas.unwrap_or_default(),
        sha3_uncles: block.uncles_hash,
    })
}