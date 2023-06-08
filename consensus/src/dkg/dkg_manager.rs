// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use std::{collections::HashMap, sync::Arc};
use aptos_consensus_types::common::Author;
use aptos_types::{transaction::SignedTransaction, validator_verifier::ValidatorVerifier};
use tokio::{sync::{oneshot, mpsc}, time::Interval};
use crate::{quorum_store::batch_generator::BatchGeneratorCommand, block_storage::BlockReader};

#[derive(Debug)]
pub struct StakeDis {
    pub distribution: HashMap<Author, u64>,
}

#[derive(Debug, Clone)]
pub struct Transcript {
    // dkg todo: use real transcript
    bytes: Vec<u8>,
}

// the transcript size is 3.25MB
const TRANSCRIPT_SIZE: usize = 3250000;

impl Transcript {
    pub fn new() -> Self {
        Transcript { bytes: vec![u8::MAX; TRANSCRIPT_SIZE] }
    }

    pub fn verify(&self) -> anyhow::Result<()> {
        // dkg todo: verify the transcript
        Ok(())
    }
}

#[derive(Debug)]
pub enum DKGManagerCommand {
    // parameters: new stake distribution
    ComputePVSS(StakeDis),
    ReceivePVSS(Author, Transcript),
    Shutdown(futures_channel::oneshot::Sender<()>),
}

pub struct DKGManager {
    epoch: u64,
    author: Author,
    old_validators: ValidatorVerifier,
    my_pvss: Option<Transcript>,
    // HashMap of valid PVSS transcripts received from other validators
    all_pvss: HashMap<Author, Transcript>,
    // Aggregated PVSS transcript from enough validators
    aggregated_pvss: Option<Transcript>,
    // dkg todo: add the key pair to sign the PVSS transcript
    // Channel to send the aggregated PVSS transcript to the batch generator
    batch_generator_cmd_tx: mpsc::Sender<BatchGeneratorCommand>,
}

impl DKGManager {
    pub fn new(
        epoch: u64,
        author: Author,
        old_validators: ValidatorVerifier,
        batch_generator_cmd_tx: mpsc::Sender<BatchGeneratorCommand>,
    ) -> Self {
        Self {
            epoch,
            author,
            old_validators,
            my_pvss: None,
            all_pvss: HashMap::new(),
            aggregated_pvss: None,
            batch_generator_cmd_tx,
        }
    }

    fn compute_pvss(&mut self, stake_dis: StakeDis) -> anyhow::Result<()> {
        // dkg todo: compute pvss transcript
        self.my_pvss = Some(Transcript::new());
        Ok(())
    }

    async fn broadcast_pvss(&self) {
        // dkg todo: reliably broadcast pvss transcript, need to ensure all validators receive it
        // waiting for the reliable broadcast implementation on main
    }

    fn aggregate_pvss(&self) -> Option<Transcript> {
        // dkg todo: aggregate all pvss transcripts
        None
    }

    pub async fn start(
        mut self,
        mut rx: tokio::sync::mpsc::Receiver<DKGManagerCommand>,
    ) {
        loop {
            tokio::select! {
                Some(msg) = rx.recv() => {
                    match msg {
                        DKGManagerCommand::ComputePVSS(stake_dis) => {
                            if self.my_pvss.is_some() {
                                // If we already have a PVSS transcript for this epoch, ignore
                                continue;
                            }
                            // dkg todo: start PVSS generation, once done reliably multicast to all validators
                            if self.compute_pvss(stake_dis).is_ok() {
                                self.all_pvss.insert(self.author, self.my_pvss.clone().unwrap());
                                self.broadcast_pvss().await;
                            }
                        }
                        DKGManagerCommand::ReceivePVSS(peer, transcript) => {
                            // dkg todo: verify if the PVSS transcript is valid
                            if transcript.verify().is_ok() && !self.all_pvss.contains_key(&peer) {
                                self.all_pvss.insert(peer, transcript);
                                if self.old_validators.check_voting_power(self.all_pvss.keys()).is_ok() {
                                    // dkg todo: aggregate PVSS transcripts from other validators
                                    if let Some(aggregated_pvss) = self.aggregate_pvss() {
                                        // dkg todo: generate a new transaction for the aggregated pvss transcript
                                        // dkg todo: send aggregated PVSS transcript to batch generator
                                        self.batch_generator_cmd_tx.send(BatchGeneratorCommand::SendPVSSBatch(None)).await.unwrap();
                                    }
                                }
                            }
                        }
                        DKGManagerCommand::Shutdown(ack_tx) => {
                            ack_tx.send(()).expect("Failed to send shutdown ack to round manager");
                            break;
                        }
                    }
                }
            }
        }
    }
}
