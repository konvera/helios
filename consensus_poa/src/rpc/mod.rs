use async_trait::async_trait;
use ethers::types::H256;
use common::types::Block;

use eyre::Result;

pub mod http_rpc;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait ExecutionRpc: Send + Clone + Sync + 'static {
    fn new(rpc: &str) -> Result<Self>
    where
        Self: Sized;

    async fn get_block_number(&self) -> Result<u64>;
    async fn get_block_by_number(&self, number: u64) -> Result<Block>;
    async fn get_block_by_hash(&self, hash: H256) -> Result<Block>;
    async fn get_latest_block(&self) -> Result<Block>;

}
