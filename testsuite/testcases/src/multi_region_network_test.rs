// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use crate::{public_fullnode_performance::create_and_add_pfns, LoadDestination, NetworkLoadTest};
use aptos_forge::{GroupNetEm, NetworkContext, NetworkTest, Swarm, SwarmChaos, SwarmNetEm, Test};
use aptos_logger::info;
use aptos_types::PeerId;
use itertools::{self, Itertools};
use rand::{seq::SliceRandom, thread_rng};
use std::collections::BTreeMap;

/// The link stats are obtained from https://github.com/doitintl/intercloud-throughput/blob/master/results_202202/results.csv
/// The four regions were hand-picked from the dataset to simulate a multi-region setup
/// with high latencies and low bandwidth.
macro_rules! FOUR_REGION_LINK_STATS_CSV {
    () => {
        "data/four_region_link_stats.csv"
    };
}

fn get_link_stats_table() -> BTreeMap<String, BTreeMap<String, (u64, f64)>> {
    let mut stats_table = BTreeMap::new();

    let mut rdr =
        csv::Reader::from_reader(include_bytes!(FOUR_REGION_LINK_STATS_CSV!()).as_slice());
    rdr.deserialize()
        .for_each(|result: Result<(String, String, u64, f64), _>| {
            if let Ok((from, to, bitrate, latency)) = result {
                stats_table
                    .entry(from)
                    .or_insert_with(BTreeMap::new)
                    .insert(to, (bitrate, latency));
            }
        });
    stats_table
}

pub(crate) fn chunk_peers(peers: Vec<PeerId>, num_groups: usize) -> Vec<Vec<PeerId>> {
    let approx_chunk_size = peers.len() / num_groups;

    let chunks = peers.chunks_exact(approx_chunk_size);

    let mut peer_chunks: Vec<Vec<PeerId>> = chunks.clone().map(|chunk| chunk.to_vec()).collect();

    // Get any remaining peers and add them to the first group
    let remaining_peers: Vec<PeerId> = chunks
        .remainder()
        .iter()
        // If `approx_validators_per_region` is 1, then it is possible we will have more regions than desired, so the
        // remaining validators will be in the first group.
        .chain(chunks.skip(num_groups).flatten())
        .cloned()
        .collect();
    if !remaining_peers.is_empty() {
        peer_chunks[0].append(remaining_peers.to_vec().as_mut());
    }

    peer_chunks
}

/// Creates a table of peers grouped by region. The peers are divided into N groups, where N is the number of regions
/// provided in the link stats table. Any remaining peers are added to the first group.
fn create_link_stats_table_with_peer_groups(
    peers: Vec<PeerId>,
    link_stats_table: &LinkStatsTable,
) -> LinkStatsTableWithPeerGroups {
    assert!(peers.len() >= link_stats_table.len());

    let number_of_regions = link_stats_table.len();
    assert!(
        number_of_regions >= 2,
        "At least 2 regions are required for inter-region network chaos."
    );
    assert!(
        number_of_regions <= 4,
        "ChaosMesh only supports simulating up to 4 regions."
    );

    let peer_chunks = chunk_peers(peers, number_of_regions);

    let peer_groups = peer_chunks
        .into_iter()
        .zip(link_stats_table.iter())
        .map(|(chunk, (from_region, stats))| (from_region.clone(), chunk, stats.clone()))
        .collect();

    peer_groups
}

// A map of "source" regions to a map of "destination" region to (bandwidth, latency)
type LinkStatsTable = BTreeMap<String, BTreeMap<String, (u64, f64)>>;
// A map of "source" regions to a tuple of (list of peers, map of "destination" region to (bandwidth, latency))
type LinkStatsTableWithPeerGroups = Vec<(String, Vec<PeerId>, BTreeMap<String, (u64, f64)>)>;

#[derive(Clone)]
pub struct InterRegionNetEmConfig {
    delay_jitter_ms: u64,
    delay_correlation_percentage: u64,
    loss_percentage: u64,
    loss_correlation_percentage: u64,
}

impl Default for InterRegionNetEmConfig {
    fn default() -> Self {
        Self {
            delay_jitter_ms: 20,
            delay_correlation_percentage: 50,
            loss_percentage: 3,
            loss_correlation_percentage: 50,
        }
    }
}

impl InterRegionNetEmConfig {
    // Creates GroupNetEm for inter-region network chaos
    fn build(&self, peer_groups: &LinkStatsTableWithPeerGroups) -> Vec<GroupNetEm> {
        let group_netems: Vec<GroupNetEm> = peer_groups
            .iter()
            .combinations(2)
            .map(|comb| {
                let (from_region, from_chunk, stats) = &comb[0];
                let (to_region, to_chunk, _) = &comb[1];

                let (bandwidth, latency) = stats.get(to_region).unwrap();
                let netem = GroupNetEm {
                    name: format!("{}-to-{}-netem", from_region, to_region),
                    source_nodes: from_chunk.to_vec(),
                    target_nodes: to_chunk.to_vec(),
                    delay_latency_ms: *latency as u64,
                    delay_jitter_ms: self.delay_jitter_ms,
                    delay_correlation_percentage: self.delay_correlation_percentage,
                    loss_percentage: self.loss_percentage,
                    loss_correlation_percentage: self.loss_correlation_percentage,
                    rate_in_mbps: *bandwidth / 1e6 as u64,
                };
                info!("inter-region netem {:?}", netem);

                netem
            })
            .collect();

        group_netems
    }
}

#[derive(Clone)]
pub struct IntraRegionNetEmConfig {
    bandwidth_rate_mbps: u64,
    delay_latency_ms: u64,
    delay_jitter_ms: u64,
    delay_correlation_percentage: u64,
    loss_percentage: u64,
    loss_correlation_percentage: u64,
}

impl Default for IntraRegionNetEmConfig {
    fn default() -> Self {
        Self {
            bandwidth_rate_mbps: 10 * 1000, // 10 Gbps
            delay_latency_ms: 50,
            delay_jitter_ms: 5,
            delay_correlation_percentage: 50,
            loss_percentage: 1,
            loss_correlation_percentage: 50,
        }
    }
}

impl IntraRegionNetEmConfig {
    fn build(&self, peer_groups: LinkStatsTableWithPeerGroups) -> Vec<GroupNetEm> {
        let group_netems: Vec<GroupNetEm> = peer_groups
            .iter()
            .map(|(region, chunk, _)| {
                let netem = GroupNetEm {
                    name: format!("{}-self-netem", region),
                    source_nodes: chunk.to_vec(),
                    target_nodes: chunk.to_vec(),
                    delay_latency_ms: self.delay_latency_ms,
                    delay_jitter_ms: self.delay_jitter_ms,
                    delay_correlation_percentage: self.delay_correlation_percentage,
                    loss_percentage: self.loss_percentage,
                    loss_correlation_percentage: self.loss_correlation_percentage,
                    rate_in_mbps: self.bandwidth_rate_mbps,
                };
                info!("intra-region netem {:?}", netem);

                netem
            })
            .collect();

        group_netems
    }
}

#[derive(Clone)]
pub struct MultiRegionNetworkEmulationConfig {
    pub link_stats_table: LinkStatsTable,
    pub inter_region_config: InterRegionNetEmConfig,
    pub intra_region_config: Option<IntraRegionNetEmConfig>,
}

impl Default for MultiRegionNetworkEmulationConfig {
    fn default() -> Self {
        Self {
            link_stats_table: get_link_stats_table(),
            inter_region_config: InterRegionNetEmConfig::default(),
            intra_region_config: Some(IntraRegionNetEmConfig::default()),
        }
    }
}

/// A test to emulate network conditions for a multi-region setup.
pub struct MultiRegionNetworkEmulationTest {
    pub add_chaos_to_validators: bool,
    pub add_chaos_to_fullnodes: bool,
    pub override_emulation_config: Option<MultiRegionNetworkEmulationConfig>,
}

/// The default implementation is typically used by most tests
impl Default for MultiRegionNetworkEmulationTest {
    fn default() -> Self {
        MultiRegionNetworkEmulationTest {
            add_chaos_to_validators: true,
            add_chaos_to_fullnodes: false,
            override_emulation_config: None,
        }
    }
}

impl MultiRegionNetworkEmulationTest {
    pub fn new(
        add_chaos_to_validators: bool,
        add_chaos_to_fullnodes: bool,
        override_emulation_config: Option<MultiRegionNetworkEmulationConfig>,
    ) -> Self {
        MultiRegionNetworkEmulationTest {
            add_chaos_to_validators,
            add_chaos_to_fullnodes,
            override_emulation_config,
        }
    }

    fn create_netem_chaos(&self, swarm: &mut dyn Swarm) -> SwarmNetEm {
        // Ensure that chaos is being added somewhere
        if !self.add_chaos_to_validators && !self.add_chaos_to_fullnodes {
            panic!("No chaos is being added to the network! Use a different test?");
        }

        // Determine the emulation config to use
        let emulation_config = self.override_emulation_config.clone().unwrap_or_default();

        // Identify the validator peer IDs for chaos
        let validators_to_add_chaos = if self.add_chaos_to_validators {
            swarm.validators().map(|v| v.peer_id()).collect::<Vec<_>>()
        } else {
            vec![]
        };

        // Identify the fullnode peer IDs for chaos (includes validator fullnodes and public fullnodes)
        let fullnodes_to_add_chaos = if self.add_chaos_to_fullnodes {
            swarm.full_nodes().map(|v| v.peer_id()).collect::<Vec<_>>()
        } else {
            vec![]
        };

        // Combine all of the peer IDs that we should add chaos to.
        // Note: we shuffle the peer IDs to ensure that the chaos
        // is distributed randomly across the node types.
        let mut peer_ids_for_chaos = validators_to_add_chaos
            .into_iter()
            .chain(fullnodes_to_add_chaos)
            .collect::<Vec<_>>();
        peer_ids_for_chaos.shuffle(&mut thread_rng());

        // Create the network chaos
        create_multi_region_swarm_network_chaos(peer_ids_for_chaos, &emulation_config)
    }
}

impl Test for MultiRegionNetworkEmulationTest {
    fn name(&self) -> &'static str {
        "network:multi-region-network-emulation"
    }
}

fn create_multi_region_swarm_network_chaos(
    all_peers: Vec<PeerId>,
    config: &MultiRegionNetworkEmulationConfig,
) -> SwarmNetEm {
    let peer_groups = create_link_stats_table_with_peer_groups(all_peers, &config.link_stats_table);

    let inter_region_netem = config.inter_region_config.build(&peer_groups);
    let intra_region_netem = config
        .intra_region_config
        .as_ref()
        .map(|config| config.build(peer_groups))
        .unwrap_or_default();

    SwarmNetEm {
        group_netems: itertools::concat(vec![intra_region_netem, inter_region_netem]),
    }
}

impl NetworkLoadTest for MultiRegionNetworkEmulationTest {
    fn setup(&self, ctx: &mut NetworkContext) -> anyhow::Result<LoadDestination> {
        // Add the PFNs to the swarm
        let num_pfns = 10;
        let pfn_peer_ids = create_and_add_pfns(ctx, num_pfns)?;

        // Inject the network chaos
        let chaos = self.create_netem_chaos(ctx.swarm());
        ctx.swarm().inject_chaos(SwarmChaos::NetEm(chaos))?;

        // Use the PFNs as the load destination
        Ok(LoadDestination::Peers(pfn_peer_ids))
    }

    fn finish(&self, swarm: &mut dyn Swarm) -> anyhow::Result<()> {
        let chaos = self.create_netem_chaos(swarm);
        swarm.remove_chaos(SwarmChaos::NetEm(chaos))
    }
}

impl NetworkTest for MultiRegionNetworkEmulationTest {
    fn run(&self, ctx: &mut NetworkContext<'_>) -> anyhow::Result<()> {
        <dyn NetworkLoadTest>::run(self, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    #[test]
    fn test_create_multi_region_swarm_network_chaos() {
        aptos_logger::Logger::new().init();

        let config = MultiRegionNetworkEmulationConfig::default();

        let all_validators = (0..8).map(|_| PeerId::random()).collect();
        let netem = create_multi_region_swarm_network_chaos(all_validators, &config);

        assert_eq!(netem.group_netems.len(), 10);

        let all_validators: Vec<PeerId> = (0..10).map(|_| PeerId::random()).collect();
        let netem = create_multi_region_swarm_network_chaos(all_validators.clone(), &config);

        assert_eq!(netem.group_netems.len(), 10);
        assert_eq!(netem.group_netems[0].source_nodes.len(), 4);
        assert_eq!(netem.group_netems[0].target_nodes.len(), 4);
        assert_eq!(netem.group_netems[0], GroupNetEm {
            name: "aws--ap-northeast-1-self-netem".to_owned(),
            rate_in_mbps: 10000,
            source_nodes: vec![
                all_validators[0],
                all_validators[1],
                all_validators[8],
                all_validators[9],
            ],
            target_nodes: vec![
                all_validators[0],
                all_validators[1],
                all_validators[8],
                all_validators[9],
            ],
            delay_latency_ms: 50,
            delay_jitter_ms: 5,
            delay_correlation_percentage: 50,
            loss_percentage: 1,
            loss_correlation_percentage: 50
        })
    }
}
