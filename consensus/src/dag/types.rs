// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use aptos_consensus_types::node::{
    CertifiedNodeAck, NodeCertificate, SignedNodeDigest, SignedNodeDigestError,
    SignedNodeDigestInfo,
};
use aptos_crypto::{bls12381, HashValue};
use aptos_types::aggregate_signature::PartialSignatures;
use aptos_types::validator_verifier::ValidatorVerifier;
use aptos_types::PeerId;
use std::collections::{BTreeMap, HashSet};

#[allow(dead_code)]
pub(crate) struct IncrementalNodeCertificateState {
    signed_node_digest_info: SignedNodeDigestInfo,
    aggregated_signature: BTreeMap<PeerId, bls12381::Signature>,
}

#[allow(dead_code)]
impl IncrementalNodeCertificateState {
    pub fn new(digest: HashValue) -> Self {
        Self {
            signed_node_digest_info: SignedNodeDigestInfo::new(digest),
            aggregated_signature: BTreeMap::new(),
        }
    }

    pub(crate) fn missing_peers_signatures(
        &self,
        validator_verifier: &ValidatorVerifier,
    ) -> Vec<PeerId> {
        let all_peers: HashSet<&PeerId> = validator_verifier
            .address_to_validator_index()
            .keys()
            .collect();
        let singers: HashSet<&PeerId> = self.aggregated_signature.keys().collect();
        all_peers.difference(&singers).cloned().cloned().collect()
    }

    //Signature we already verified
    pub(crate) fn add_signature(
        &mut self,
        signed_node_digest: SignedNodeDigest,
    ) -> Result<(), SignedNodeDigestError> {
        if signed_node_digest.info() != &self.signed_node_digest_info {
            return Err(SignedNodeDigestError::WrongDigest);
        }

        if self
            .aggregated_signature
            .contains_key(&signed_node_digest.peer_id())
        {
            return Err(SignedNodeDigestError::DuplicatedSignature);
        }

        self.aggregated_signature
            .insert(signed_node_digest.peer_id(), signed_node_digest.signature());
        Ok(())
    }

    pub(crate) fn ready(&self, validator_verifier: &ValidatorVerifier) -> bool {
        validator_verifier
            .check_voting_power(self.aggregated_signature.keys())
            .is_ok()
    }

    pub(crate) fn take(self, validator_verifier: &ValidatorVerifier) -> NodeCertificate {
        let proof = match validator_verifier
            .aggregate_signatures(&PartialSignatures::new(self.aggregated_signature))
        {
            Ok(sig) => NodeCertificate::new(self.signed_node_digest_info, sig),
            Err(e) => unreachable!("Cannot aggregate signatures on digest err = {:?}", e),
        };
        proof
    }
}

pub(crate) struct AckSet {
    digest: HashValue,
    set: HashSet<PeerId>,
}

impl AckSet {
    pub fn new(digest: HashValue) -> Self {
        Self {
            digest,
            set: HashSet::new(),
        }
    }

    pub fn add(&mut self, ack: CertifiedNodeAck) {
        if ack.digest() == self.digest {
            self.set.insert(ack.peer_id());
        }
    }

    pub fn missing_peers(&self, verifier: &ValidatorVerifier) -> Vec<PeerId> {
        let all_peers: HashSet<PeerId> = verifier
            .address_to_validator_index()
            .keys()
            .cloned()
            .collect();
        all_peers.difference(&self.set).cloned().collect()
    }
}