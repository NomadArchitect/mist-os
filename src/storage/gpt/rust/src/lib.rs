// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, Context as _, Error};
use block_client::{BlockClient as _, MutableBufferSlice, RemoteBlockClient};
use std::collections::BTreeMap;
use zerocopy::FromBytes as _;

pub mod format;

/// GPT GUIDs are stored in mixed-endian format (see Appendix A of the EFI spec).  To ensure this is
/// correctly handled, wrap the Uuid type to hide methods that use the UUIDs inappropriately.
#[derive(Clone, Default, Debug)]
pub struct Guid(uuid::Uuid);

impl From<uuid::Uuid> for Guid {
    fn from(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }
}

impl Guid {
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(uuid::Uuid::from_bytes_le(bytes))
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        self.0.to_bytes_le()
    }

    pub fn to_string(&self) -> String {
        self.0.to_string()
    }
}

#[derive(Clone, Debug)]
pub struct PartitionInfo {
    pub label: String,
    pub type_guid: Guid,
    pub instance_guid: Guid,
    pub start_block: u64,
    pub num_blocks: u64,
}

impl PartitionInfo {
    fn from_entry(entry: &format::PartitionTableEntry) -> Result<Self, Error> {
        let label =
            String::from_utf16(entry.name.split(|v| *v == 0u16).next().unwrap())?.to_owned();
        Ok(Self {
            label,
            type_guid: Guid::from_bytes(entry.type_guid),
            instance_guid: Guid::from_bytes(entry.instance_guid),
            start_block: entry.first_lba,
            num_blocks: entry
                .last_lba
                .checked_add(1)
                .unwrap()
                .checked_sub(entry.first_lba)
                .unwrap(),
        })
    }
}

enum WhichHeader {
    Primary,
    Backup,
}

impl WhichHeader {
    fn offset(&self, block_size: u64, block_count: u64) -> u64 {
        match self {
            Self::Primary => block_size,
            Self::Backup => (block_count - 1) * block_size,
        }
    }
}

async fn load_metadata(
    client: &RemoteBlockClient,
    which: WhichHeader,
) -> Result<(format::Header, BTreeMap<u32, PartitionInfo>), Error> {
    let bs = client.block_size() as usize;
    let mut header_block = vec![0u8; client.block_size() as usize];
    client
        .read_at(
            MutableBufferSlice::Memory(&mut header_block[..]),
            which.offset(bs as u64, client.block_count() as u64),
        )
        .await
        .context("Read header")?;
    let header = format::Header::ref_from_prefix(&header_block[..])
        .ok_or(anyhow!("Header has invalid size"))?;
    header.ensure_integrity(client.block_count(), client.block_size() as u64)?;
    let partition_table_offset = header.part_start * bs as u64;
    let partition_table_size = (header.num_parts * header.part_size) as usize;
    let partition_table_size_rounded = partition_table_size
        .checked_next_multiple_of(bs)
        .ok_or(anyhow!("Overflow when rounding up partition table size "))?;
    let mut partition_table = BTreeMap::new();
    if header.num_parts > 0 {
        let mut partition_table_blocks = vec![0u8; partition_table_size_rounded];
        client
            .read_at(
                MutableBufferSlice::Memory(&mut partition_table_blocks[..]),
                partition_table_offset,
            )
            .await
            .with_context(|| {
                format!(
                    "Failed to read partition table (sz {}) from offset {}",
                    partition_table_size, partition_table_offset
                )
            })?;
        let crc = crc::crc32::checksum_ieee(&partition_table_blocks[..partition_table_size]);
        anyhow::ensure!(header.crc32_parts == crc, "Invalid partition table checksum");

        for i in 0..header.num_parts as usize {
            let entry_raw = &partition_table_blocks
                [i * header.part_size as usize..(i + 1) * header.part_size as usize];
            let entry = format::PartitionTableEntry::ref_from_prefix(entry_raw)
                .ok_or(anyhow!("Failed to parse partition {i}"))?;
            if entry.is_empty() {
                continue;
            }
            entry.ensure_integrity().context("GPT partition table entry invalid!")?;

            partition_table.insert(i as u32, PartitionInfo::from_entry(entry)?);
        }
    }
    Ok((header.clone(), partition_table))
}

/// Manages a connection to a GPT-formatted block device.
pub struct GptManager {
    _client: RemoteBlockClient,
    header: format::Header,
    partitions: BTreeMap<u32, PartitionInfo>,
}

impl std::fmt::Debug for GptManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("GptManager")
            .field("header", &self.header)
            .field("partitions", &self.partitions)
            .finish()
    }
}

impl GptManager {
    /// Loads and validates a GPT-formatted block device.
    pub async fn open(client: RemoteBlockClient) -> Result<Self, Error> {
        let (header, partitions) = match load_metadata(&client, WhichHeader::Primary).await {
            Ok(v) => v,
            Err(error) => {
                tracing::warn!(?error, "Failed to load primary metadata; falling back to backup");
                load_metadata(&client, WhichHeader::Backup)
                    .await
                    .context("Failed to load backup metadata")?
            }
        };
        Ok(Self { _client: client, header, partitions })
    }

    pub fn header(&self) -> &format::Header {
        &self.header
    }

    pub fn partitions(&self) -> &BTreeMap<u32, PartitionInfo> {
        &self.partitions
    }
}

#[cfg(test)]
mod tests {
    use crate::GptManager;
    use block_client::RemoteBlockClient;
    use fake_block_server::FakeServer;
    use futures::FutureExt as _;
    use gpt_testing::{format_gpt, PartitionDescriptor};
    use std::ops::Range;
    use std::sync::Arc;
    use {fidl_fuchsia_hardware_block_volume as fvolume, fuchsia_zircon as zx};

    #[fuchsia::test]
    async fn load_unformatted_gpt() {
        let vmo = zx::Vmo::create(4096).unwrap();
        let server = Arc::new(FakeServer::from_vmo(512, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect_err("load should fail");
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_formatted_empty_gpt() {
        let vmo = zx::Vmo::create(4096).unwrap();
        format_gpt(&vmo, 512, vec![]);
        let server = Arc::new(FakeServer::from_vmo(512, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_minimal_size() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part";

        let vmo = zx::Vmo::create(6 * 4096).unwrap();
        format_gpt(
            &vmo,
            4096,
            vec![PartitionDescriptor {
                label: PART_NAME.to_string(),
                type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_GUID),
                start_block: 3,
                num_blocks: 1,
            }],
        );
        let server = Arc::new(FakeServer::from_vmo(4096, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                let partition = manager.partitions().get(&0).expect("No entry found");
                assert_eq!(partition.start_block, 3);
                assert_eq!(partition.num_blocks, 1);
                assert!(manager.partitions().get(&1).is_none());
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_one_partition() {
        const PART_TYPE_GUID: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        const PART_INSTANCE_GUID: [u8; 16] =
            [16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31];
        const PART_NAME: &str = "part";

        let vmo = zx::Vmo::create(4096).unwrap();
        format_gpt(
            &vmo,
            512,
            vec![PartitionDescriptor {
                label: PART_NAME.to_string(),
                type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
            }],
        );
        let server = Arc::new(FakeServer::from_vmo(512, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                let partition = manager.partitions().get(&0).expect("No entry found");
                assert_eq!(partition.label, "part");
                assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
                assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_GUID);
                assert_eq!(partition.start_block, 4);
                assert_eq!(partition.num_blocks, 1);
                assert!(manager.partitions().get(&1).is_none());
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_two_partitions() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let vmo = zx::Vmo::create(8192).unwrap();
        format_gpt(
            &vmo,
            512,
            vec![
                PartitionDescriptor {
                    label: PART_1_NAME.to_string(),
                    type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                    instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                },
                PartitionDescriptor {
                    label: PART_2_NAME.to_string(),
                    type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                    instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 7,
                    num_blocks: 1,
                },
            ],
        );
        let server = Arc::new(FakeServer::from_vmo(512, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                let partition = manager.partitions().get(&0).expect("No entry found");
                assert_eq!(partition.label, PART_1_NAME);
                assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
                assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
                assert_eq!(partition.start_block, 4);
                assert_eq!(partition.num_blocks, 1);
                let partition = manager.partitions().get(&1).expect("No entry found");
                assert_eq!(partition.label, PART_2_NAME);
                assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
                assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
                assert_eq!(partition.start_block, 7);
                assert_eq!(partition.num_blocks, 1);
                assert!(manager.partitions().get(&2).is_none());
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_extra_bytes_in_partition_name() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part\0extrastuff";

        let vmo = zx::Vmo::create(4096).unwrap();
        format_gpt(
            &vmo,
            512,
            vec![PartitionDescriptor {
                label: PART_NAME.to_string(),
                type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
            }],
        );
        let server = Arc::new(FakeServer::from_vmo(512, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                let partition = manager.partitions().get(&0).expect("No entry found");
                // The name should have everything after the first nul byte stripped.
                assert_eq!(partition.label, "part");
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_empty_partition_name() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "";

        let vmo = zx::Vmo::create(4096).unwrap();
        format_gpt(
            &vmo,
            512,
            vec![PartitionDescriptor {
                label: PART_NAME.to_string(),
                type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
            }],
        );
        let server = Arc::new(FakeServer::from_vmo(512, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                let partition = manager.partitions().get(&0).expect("No entry found");
                assert_eq!(partition.label, "");
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_invalid_primary_header() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let vmo = zx::Vmo::create(8192).unwrap();
        format_gpt(
            &vmo,
            512,
            vec![
                PartitionDescriptor {
                    label: PART_1_NAME.to_string(),
                    type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                    instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                },
                PartitionDescriptor {
                    label: PART_2_NAME.to_string(),
                    type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                    instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 7,
                    num_blocks: 1,
                },
            ],
        );
        // Clobber the primary header.  The backup should allow the GPT to be used.
        vmo.write(&[0xffu8; 512], 512).unwrap();

        let server = Arc::new(FakeServer::from_vmo(512, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                let partition = manager.partitions().get(&0).expect("No entry found");
                assert_eq!(partition.label, PART_1_NAME);
                assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
                assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
                assert_eq!(partition.start_block, 4);
                assert_eq!(partition.num_blocks, 1);
                let partition = manager.partitions().get(&1).expect("No entry found");
                assert_eq!(partition.label, PART_2_NAME);
                assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
                assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
                assert_eq!(partition.start_block, 7);
                assert_eq!(partition.num_blocks, 1);
                assert!(manager.partitions().get(&2).is_none());
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_invalid_primary_partition_table() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let vmo = zx::Vmo::create(8192).unwrap();
        format_gpt(
            &vmo,
            512,
            vec![
                PartitionDescriptor {
                    label: PART_1_NAME.to_string(),
                    type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                    instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                },
                PartitionDescriptor {
                    label: PART_2_NAME.to_string(),
                    type_guid: uuid::Uuid::from_bytes(PART_TYPE_GUID),
                    instance_guid: uuid::Uuid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 7,
                    num_blocks: 1,
                },
            ],
        );
        // Clobber the primary partition table.  The backup should allow the GPT to be used.
        vmo.write(&[0xffu8; 512], 1024).unwrap();

        let server = Arc::new(FakeServer::from_vmo(512, vmo));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                let partition = manager.partitions().get(&0).expect("No entry found");
                assert_eq!(partition.label, PART_1_NAME);
                assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
                assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
                assert_eq!(partition.start_block, 4);
                assert_eq!(partition.num_blocks, 1);
                let partition = manager.partitions().get(&1).expect("No entry found");
                assert_eq!(partition.label, PART_2_NAME);
                assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
                assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
                assert_eq!(partition.start_block, 7);
                assert_eq!(partition.num_blocks, 1);
                assert!(manager.partitions().get(&2).is_none());
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_golden_gpt_linux() {
        let contents = std::fs::read("/pkg/data/gpt_golden/gpt.linux.blk").unwrap();
        let server = Arc::new(FakeServer::new(contents.len() as u64 / 512, 512, &contents));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                let partition = manager.partitions().get(&0).expect("No entry found");
                assert_eq!(partition.label, "ext");
                assert_eq!(partition.type_guid.to_string(),
                    "0fc63daf-8483-4772-8e79-3d69d8477de4");
                assert_eq!(partition.start_block, 8);
                assert_eq!(partition.num_blocks, 1);
                assert!(manager.partitions().get(&1).is_none());
            }.fuse() => {},
        );
    }

    #[fuchsia::test]
    async fn load_golden_gpt_fuchsia() {
        let contents = std::fs::read("/pkg/data/gpt_golden/gpt.fuchsia.blk").unwrap();
        let server = Arc::new(FakeServer::new(contents.len() as u64 / 512, 512, &contents));
        let (client, server_end) =
            fidl::endpoints::create_proxy::<fvolume::VolumeMarker>().unwrap();

        struct ExpectedPartition {
            label: &'static str,
            type_guid: &'static str,
            blocks: Range<u64>,
        }
        const EXPECTED_PARTITIONS: [ExpectedPartition; 8] = [
            ExpectedPartition {
                label: "fuchsia-esp",
                type_guid: "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
                blocks: 11..12,
            },
            ExpectedPartition {
                label: "zircon-a",
                type_guid: "de30cc86-1f4a-4a31-93c4-66f147d33e05",
                blocks: 12..13,
            },
            ExpectedPartition {
                label: "zircon-b",
                type_guid: "23cc04df-c278-4ce7-8471-897d1a4bcdf7",
                blocks: 13..14,
            },
            ExpectedPartition {
                label: "zircon-r",
                type_guid: "a0e5cf57-2def-46be-a80c-a2067c37cd49",
                blocks: 14..15,
            },
            ExpectedPartition {
                label: "vbmeta_a",
                type_guid: "a13b4d9a-ec5f-11e8-97d8-6c3be52705bf",
                blocks: 15..16,
            },
            ExpectedPartition {
                label: "vbmeta_b",
                type_guid: "a288abf2-ec5f-11e8-97d8-6c3be52705bf",
                blocks: 16..17,
            },
            ExpectedPartition {
                label: "vbmeta_r",
                type_guid: "6a2460c3-cd11-4e8b-80a8-12cce268ed0a",
                blocks: 17..18,
            },
            ExpectedPartition {
                label: "misc",
                type_guid: "1d75395d-f2c6-476b-a8b7-45cc1c97b476",
                blocks: 18..19,
            },
        ];

        futures::select!(
            _ = server.serve(server_end.into_stream().unwrap()).fuse() => {
                unreachable!();
            },
            _ = async {
                let manager = GptManager::open(RemoteBlockClient::new(client).await.unwrap())
                    .await
                    .expect("load should succeed");
                for i in 0..EXPECTED_PARTITIONS.len() as u32 {
                    let partition = manager.partitions().get(&i).expect("No entry found");
                    let expected = &EXPECTED_PARTITIONS[i as usize];
                    assert_eq!(partition.label, expected.label);
                    assert_eq!(partition.type_guid.to_string(), expected.type_guid);
                    assert_eq!(partition.start_block, expected.blocks.start);
                    assert_eq!(partition.num_blocks, expected.blocks.end - expected.blocks.start);
                }
            }.fuse() => {},
        );
    }
}
