use std::marker::PhantomData;
use std::process;
use std::sync::Arc;
use chrono::Duration;

use crate::consensus::Consensus;
use crate::rpc::ExecutionRpc;

use eyre::Result;
use ssz_rs::prelude::*;
use tokio::sync::mpsc::Sender;
use tracing::{error, warn};
use zduny_wasm_timer::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc::channel;
use tokio::sync::mpsc::Receiver;
use tokio::sync::watch;

use common::types::Block;
use config::Config;

use consensus::database::Database;

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
    consensus: Consensus,
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

impl<R: ExecutionRpc> Inner<R> {
    pub fn new(
        rpc: &str,
        block_send: Sender<Block>,
        finalized_block_send: watch::Sender<Option<Block>>,
        config: Arc<Config>,
    ) -> Result<Self> {
        let rpc = R::new(rpc)?;

        let consensus = Consensus::new()?;

        Ok(Inner {
            rpc,
            block_send,
            finalized_block_send,
            consensus,
            config,
        })
    }

    pub async fn sync(&mut self) -> Result<()> {
        let block = self.rpc.get_latest_block().await?;

        self.consensus.sync(block)
    }

    /// Gets the duration until the next update
    /// Updates are scheduled for 2 seconds into each slot
    pub fn duration_until_next_update(&self) -> Duration {

        let mut next_update = 1;
        if let Some(latest_block) = &self.consensus.latest_block {
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
    
    pub async fn send_blocks(&self) -> Result<()> {

        if let Some(latest_block) = &self.consensus.latest_block {
            self.block_send.send(latest_block.clone()).await?;
        }

        Ok(())
    }

    pub async fn advance(&mut self) -> Result<()> {
        let next_block_number = self.consensus.latest_block.as_ref().unwrap().number.as_u64()+1;
        let block = self.rpc.get_block_by_number(next_block_number).await?;

        self.consensus.advance(block)
    }
}