use std::marker::PhantomData;
use std::process;
use std::sync::Arc;
use chrono::Duration;
use consensus::errors::ConsensusError;
use reth_primitives::B64;
use reth_primitives::Bloom;
use reth_primitives::hex::FromHex;
use reth_primitives::revm_primitives::FixedBytes;

use crate::rpc::ExecutionRpc;

use eyre::Result;
use ssz_rs::prelude::*;
use tokio::sync::mpsc::Sender;
use tracing::{error, warn, info};
use zduny_wasm_timer::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc::channel;
use tokio::sync::mpsc::Receiver;
use tokio::sync::watch;

use common::types::Block;
use config::Config;

use consensus::database::Database;

use reth_primitives::{Header, Address, revm::env::recover_header_signer};

#[allow(dead_code)] 
pub struct ConsensusClientPoA<R: ExecutionRpc, DB: Database> {
    pub block_recv: Option<Receiver<Block>>,
    pub finalized_block_recv: Option<watch::Receiver<Option<Block>>>,
    genesis_time: u64,
    db: DB,
    phantom: PhantomData<R>,
}

#[derive(Debug)]
#[allow(dead_code)] 
pub struct Inner<R: ExecutionRpc> {
    rpc: R,
    block_send: Sender<Block>,
    finalized_block_send: watch::Sender<Option<Block>>,
    latest_block: Option<Block>,
    signers: Vec<Address>,
    pub config: Arc<Config>,
}

impl<R: ExecutionRpc, DB: Database> ConsensusClientPoA<R, DB> {
    pub fn new(rpc: &str, config: Arc<Config>) -> Result<ConsensusClientPoA<R, DB>> {
        let (block_send, block_recv) = channel(256);
        let (finalized_block_send, finalized_block_recv) = watch::channel(None);

        let rpc = rpc.to_string();
        let genesis_time = config.chain.genesis_time;
        let db = DB::new(&config)?;

        #[cfg(not(target_arch = "wasm32"))]
        let run = tokio::spawn;

        #[cfg(target_arch = "wasm32")]
        let run = wasm_bindgen_futures::spawn_local;

        run(async move {
            let mut inner = Inner::<R>::new(
                &rpc,
                block_send,
                finalized_block_send,
                config.clone(),
            ).unwrap();

            let res = inner.sync().await;
            if let Err(err) = res {
                error!(target: "helios::consensus", err = %err, "sync failed");
                process::exit(1);
            }

            _ = inner.send_blocks().await;

            loop {
                zduny_wasm_timer::Delay::new(inner.duration_until_next_update().to_std().unwrap())
                    .await
                    .unwrap();

                let res = inner.advance().await;
                if let Err(err) = res {
                    warn!(target: "helios::consensus", "advance error: {}", err);
                    continue;
                }

                let res = inner.send_blocks().await;
                if let Err(err) = res {
                    warn!(target: "helios::consensus", "send error: {}", err);
                    continue;
                }
            }
        });

        Ok(ConsensusClientPoA {
            block_recv: Some(block_recv),
            finalized_block_recv: Some(finalized_block_recv),
            genesis_time,
            db,
            phantom: PhantomData,
        })
    }

    pub fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    pub fn expected_current_slot(&self) -> u64 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let since_genesis = now - std::time::Duration::from_secs(self.genesis_time);

        since_genesis.as_secs() / 12
    }
}

fn verify(block: &Block) -> Address {
    let header = extract_header(&block).unwrap();

    // TODO: can this kill the client if it recieves a malicous block?
    let creator: Address = recover_header_signer(&header).unwrap_or_else(|err| {
        panic!(
            "Failed to recover Clique Consensus signer from header ({}, {}) using extradata {}: {:?}",
            header.number, header.hash_slow(), header.extra_data, err
        )
    });

    return creator;
}

pub fn extract_header(block: &Block) -> Result<Header> {
    Ok(Header {   
        parent_hash: FixedBytes::new(block.parent_hash.into()),
        ommers_hash: FixedBytes::new(block.sha3_uncles.into()),
        beneficiary: Address::new(block.miner.into()),
        state_root: FixedBytes::new(block.state_root.into()),
        transactions_root: FixedBytes::new(block.transactions_root.into()),
        receipts_root: FixedBytes::new(block.receipts_root.into()),
        withdrawals_root: None,
        logs_bloom: Bloom { 0: FixedBytes::new(block.logs_bloom.to_vec().try_into().unwrap()) },
        timestamp: block.timestamp.as_u64(),
        mix_hash: FixedBytes::new(block.mix_hash.into()),
        nonce: u64::from_be_bytes(B64::from_hex(block.nonce.clone())?.try_into()?),
        base_fee_per_gas: None,
        number: block.number.as_u64(),
        gas_limit: block.gas_limit.as_u64(),
        difficulty: block.difficulty.into(),
        gas_used: block.gas_used.as_u64(),
        extra_data: block.extra_data.0.clone().into(),
        parent_beacon_block_root: None,
        blob_gas_used: None,
        excess_blob_gas: None,
    })
}

impl<R: ExecutionRpc> Inner<R> {
    pub fn new(
        rpc: &str,
        block_send: Sender<Block>,
        finalized_block_send: watch::Sender<Option<Block>>,
        config: Arc<Config>,
    ) -> Result<Self> {
        let rpc = R::new(rpc)?;

        let signers = [
            Address::from_hex("0x0981717712ed2c4919fdbc27dfc804800a9eeff9")?,
            Address::from_hex("0x0e5b9aa4925ed1beeb08d1c5a92477a1b719baa7")?,
            Address::from_hex("0x0e8705e07bbe1ce2c39093df3d20aaa5120bfc7a")?,
        ].to_vec();

        Ok(Inner {
            rpc,
            block_send,
            finalized_block_send,
            latest_block: None,
            config,
            signers,
        })
    }

    pub async fn sync(&mut self) -> Result<()> {

        let block = self.rpc.get_latest_block().await?;
        let creator = verify(&block);

        if !self.signers.contains(&creator) {
            error!(
                target: "helios::consensus", 
                "sync block contains invalid block creator: {}",
                creator
            );
            return Err(ConsensusError::InvalidSignature.into());
        }

        info!(
            target: "helios::consensus",
            "PoA consensus client in sync with block: {}: {:#?}",
            &block.number, &block.hash
        );

        self.latest_block = Some(block);

        Ok(())
    }


    
    pub async fn send_blocks(&self) -> Result<()> {

        if let Some(latest_block) = &self.latest_block {
            self.block_send.send(latest_block.clone()).await?;
        }

        Ok(())
    }


    /// Gets the duration until the next update
    /// Updates are scheduled for 2 seconds into each slot
    pub fn duration_until_next_update(&self) -> Duration {

        let mut next_update = 1;
        if let Some(latest_block) = &self.latest_block {
            let next_slot_timestamp = latest_block.timestamp.as_u64() + 4;

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let time_to_next_slot = next_slot_timestamp - now;
            next_update = time_to_next_slot + 2;
        }

        Duration::seconds(next_update as i64)
    }

    pub async fn advance(&mut self) -> Result<()> {
        let block = self.rpc.get_block_by_number(self.latest_block.as_ref().unwrap().number.as_u64()+1).await?;

        let creator = verify(&block);

        if !self.signers.contains(&creator) {
            error!(
                target: "helios::consensus", 
                "advance block contains invalid block creator: {}",
                creator
            );
            return Err(ConsensusError::InvalidSignature.into());
        }

        info!(
            target: "helios::consensus",
            "PoA consensus client advanced to block {}: {:#?}",
            &block.number, &block.hash
        );

        self.latest_block = Some(block);

        Ok(())
    }
}