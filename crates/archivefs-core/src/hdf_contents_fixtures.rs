//! Small generated disks for contents-view regressions; never real media.
pub fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

pub fn checksum(block: &mut [u8], offset: usize) {
    put(block, offset, 0);
    let sum = block.chunks_exact(4).fold(0_u32, |sum, word| {
        sum.wrapping_add(u32::from_be_bytes(word.try_into().unwrap()))
    });
    put(block, offset, 0_u32.wrapping_sub(sum));
}

/// Flat OFS/FFS: root block 4, a subdirectory, and the requested number of
/// files. File payloads need not exist: the UI promises header metadata only.
pub fn flat(dos: u8, files: usize) -> Vec<u8> {
    let sectors = (files + 16).max(64);
    let mut bytes = vec![0; sectors * 512];
    bytes[..3].copy_from_slice(b"DOS");
    bytes[3] = dos;
    put(&mut bytes, 8, 4);
    let root = &mut bytes[4 * 512..5 * 512];
    put(root, 0, 2);
    put(root, 12, 72);
    put(root, 24, 8);
    root[432] = 4;
    root[433..437].copy_from_slice(b"Work");
    put(root, 508, 1);
    checksum(root, 20);
    let sub = &mut bytes[8 * 512..9 * 512];
    put(sub, 0, 2);
    sub[432] = 5;
    sub[433..438].copy_from_slice(b"Games");
    put(sub, 500, 4);
    put(sub, 504, 0);
    put(sub, 508, 2);
    put(sub, 496, if files == 0 { 0 } else { 10 });
    checksum(sub, 20);
    for index in 0..files {
        let block = &mut bytes[(index + 10) * 512..(index + 11) * 512];
        put(block, 0, 2);
        put(block, 324, 37);
        let name = format!("file{index:04}");
        block[432] = name.len() as u8;
        block[433..433 + name.len()].copy_from_slice(name.as_bytes());
        put(
            block,
            496,
            if index + 1 == files {
                0
            } else {
                (index + 11) as u32
            },
        );
        put(block, 500, 4);
        put(block, 508, (-3_i32) as u32);
        checksum(block, 20);
    }
    bytes
}

/// Wrap the flat disk in one checksummed RDB/PART range.
pub fn rdb(partition: &[u8]) -> Vec<u8> {
    let sectors = partition.len() / 512;
    let mut bytes = vec![0; partition.len() * 2];
    let root = &mut bytes[..512];
    root[..4].copy_from_slice(b"RDSK");
    put(root, 4, 128);
    put(root, 16, 512);
    put(root, 28, 1);
    checksum(root, 8);
    let part = &mut bytes[512..1024];
    part[..4].copy_from_slice(b"PART");
    put(part, 4, 128);
    put(part, 16, u32::MAX);
    part[36] = 3;
    part[37..40].copy_from_slice(b"DH0");
    put(part, 140, 1);
    put(part, 148, sectors as u32);
    put(part, 164, 1);
    put(part, 168, 1);
    put(
        part,
        192,
        u32::from_be_bytes(partition[..4].try_into().unwrap()),
    );
    checksum(part, 8);
    bytes[partition.len()..].copy_from_slice(partition);
    bytes
}

pub fn two_partitions() -> Vec<u8> {
    let flat = flat(1, 2);
    let mut bytes = rdb(&flat);
    let second = bytes[512..1024].to_vec();
    bytes[1024..1536].copy_from_slice(&second);
    put(&mut bytes[512..1024], 16, 2);
    checksum(&mut bytes[512..1024], 8);
    let part = &mut bytes[1024..1536];
    part[39] = b'1';
    put(part, 164, 2);
    put(part, 168, 2);
    checksum(part, 8);
    bytes.extend_from_slice(&flat);
    bytes
}
