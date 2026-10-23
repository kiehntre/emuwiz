//! Small generated PSP ISO for analyser regressions; never real media.
//! Root: PSP_GAME/, MOVIE/INTRO.PMF (4096), DATA.BIN (2048), FRENCH.PAK (2048).

fn both32(dest: &mut [u8], value: u32) {
    dest[0..4].copy_from_slice(&value.to_le_bytes());
    dest[4..8].copy_from_slice(&value.to_be_bytes());
}

fn record(block: &mut Vec<u8>, name: &[u8], dir: bool, lba: u32, size: u32) {
    let mut length = 33 + name.len();
    if name.len().is_multiple_of(2) {
        length += 1;
    }
    let mut rec = vec![0u8; length];
    rec[0] = length as u8;
    both32(&mut rec[2..10], lba);
    both32(&mut rec[10..18], size);
    rec[25] = if dir { 2 } else { 0 };
    rec[32] = name.len() as u8;
    rec[33..33 + name.len()].copy_from_slice(name);
    block.extend(rec);
}

fn dir_block(entries: &[(&str, bool, u32, u32)]) -> Vec<u8> {
    let mut block = Vec::new();
    record(&mut block, &[0], true, 0, 2048);
    record(&mut block, &[1], true, 0, 2048);
    for (name, dir, lba, size) in entries {
        record(&mut block, name.as_bytes(), *dir, *lba, *size);
    }
    block.resize(2048, 0);
    block
}

pub fn psp_iso() -> Vec<u8> {
    let mut image = vec![0u8; 25 * 2048];
    let pvd = &mut image[16 * 2048..17 * 2048];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[40..72].fill(b' ');
    pvd[40..44].copy_from_slice(b"TEST");
    pvd[128..130].copy_from_slice(&2048u16.to_le_bytes());
    pvd[130..132].copy_from_slice(&2048u16.to_be_bytes());
    pvd[156] = 34;
    both32(&mut pvd[158..166], 18);
    both32(&mut pvd[166..174], 2048);
    pvd[181] = 2;
    pvd[188] = 1;
    let term = &mut image[17 * 2048..18 * 2048];
    term[0] = 255;
    term[1..6].copy_from_slice(b"CD001");
    term[6] = 1;
    let blocks = [
        (
            18,
            dir_block(&[
                ("PSP_GAME", true, 19, 2048),
                ("MOVIE", true, 20, 2048),
                ("DATA.BIN", false, 23, 2048),
                ("FRENCH.PAK", false, 24, 2048),
            ]),
        ),
        (19, dir_block(&[])),
        (20, dir_block(&[("INTRO.PMF", false, 21, 4096)])),
    ];
    for (lba, block) in blocks {
        image[lba * 2048..(lba + 1) * 2048].copy_from_slice(&block);
    }
    image
}
