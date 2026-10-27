use codebreaker::Codebreaker;
use std::io::Read;
fn h(s: &str) -> u32 {
    u32::from_str_radix(s, 16).unwrap()
}
fn main() {
    if std::env::args().any(|a| a == "--golden") {
        golden();
        return;
    }
    let mut t = String::new();
    std::io::stdin().read_to_string(&mut t).unwrap();
    let mut classes = std::collections::BTreeMap::new();
    let mut it = t.lines();
    let (mut streams, mut lines, mut bad) = (0, 0, 0);
    let (mut enc_bad, mut c_bad, mut compat_bad) = (0, 0, 0);
    while let Some(hd) = it.next() {
        let p: Vec<&str> = hd.split(' ').collect();
        let mode: u32 = p[0].parse().unwrap();
        let n: usize = p[1].parse().unwrap();
        let mut rows = vec![];
        for _ in 0..n {
            let f: Vec<u32> = it.next().unwrap().split(' ').map(h).collect();
            rows.push(f);
        }
        let mut crow = vec![];
        for _ in 0..n {
            let l = it.next().unwrap();
            let f: Vec<u32> = l[2..].split(' ').map(h).collect();
            crow.push((f[0], f[1]));
        }
        let mut feat = (0u32, 0u32);
        let mut first: Option<usize> = None;
        let mut cb = if mode == 1 {
            Codebreaker::new_v7()
        } else {
            Codebreaker::new()
        };
        let mut enc = if mode == 1 {
            Codebreaker::new_v7()
        } else {
            Codebreaker::new()
        };
        streams += 1;
        for (i, r) in rows.iter().enumerate() {
            if enc.encrypt_code(r[0], r[1]) != (r[2], r[3]) {
                enc_bad += 1;
            }
            if crow[i] != (r[0], r[1]) {
                c_bad += 1;
            }
            let d = cb.decrypt_code(r[2], r[3]);
            lines += 1;
            if d != crow[i] {
                compat_bad += 1;
            }
            if first.is_none() && (d != (r[0], r[1]) || d != crow[i]) {
                first = Some(i);
            }
            if r[0] == 0xBEEFC0DE {
                feat.0 += 1;
            }
            if r[0] == 0xBEEFC0DF {
                feat.1 += 1;
            }
            if d != (r[0], r[1]) || d != crow[i] {
                bad += 1;
                if bad < 6 {
                    eprintln!(
                        "MISMATCH mode{mode} line{i}: plain {:08X} {:08X} rust {:08X} {:08X} c {:08X} {:08X}",
                        r[0], r[1], d.0, d.1, crow[i].0, crow[i].1
                    );
                }
            }
        }
        let key = (mode, feat.0.min(2), feat.1.min(1), first.is_some());
        *classes.entry(key).or_insert(0u32) += 1;
    }
    for (k, v) in &classes {
        println!(
            "mode{} DE={} DF={} mismatch={} : {} streams",
            k.0, k.1, k.2, k.3, v
        );
    }
    println!(
        "streams={streams} lines={lines} mismatches={bad} encrypt_mismatches={enc_bad} c_roundtrip_mismatches={c_bad} c_compat_mismatches={compat_bad}"
    );
}

// Both exact vectors carried by upstream 75471b6, independent of round-trip generation.
fn golden() {
    let common = [
        (0xBEEFC0DE, 0, 0x8787C575, 0x1AC4C1B4),
        (0x9029BEAC, 0x0C0A9225, 0x02210430, 0x184C16E8),
        (0x201F6024, 0, 0x32E2A916, 0x7E6017BA),
        (0x2096F5B8, 0xBE, 0xCBB720FD, 0xD61505E0),
    ];
    let setup = [
        (0xBEEFC0DE, 0, 0xB4336FA9, 0x4DFEFB79),
        (0xBEEFC0DF, 0xB16B00B5, 0xFE8B8601, 0xC7C6F6CE),
        (0x01234567, 0x89ABCDEF, 0x2195D855, 0x63FA11A7),
        (0x9029BEAC, 0x0C0A9225, 0x0CA31760, 0xA6F7E88A),
        (0x201F6024, 0, 0x679DC392, 0xFA43E30B),
        (0x2096F5B8, 0xBE, 0x1CD9CCC3, 0x6AF74E36),
    ];
    let mut checks = 0;
    for (mode, rows) in [(1, &common[..]), (0, &setup[..])] {
        let mut enc = if mode == 1 {
            Codebreaker::new_v7()
        } else {
            Codebreaker::new()
        };
        let mut dec = enc.clone();
        let mut auto = enc.clone();
        for &(pa, pv, ea, ev) in rows {
            assert_eq!(enc.encrypt_code(pa, pv), (ea, ev));
            assert_eq!(dec.decrypt_code(ea, ev), (pa, pv));
            assert_eq!(auto.auto_decrypt_code(ea, ev), (pa, pv));
            checks += 3;
        }
    }
    println!("upstream golden vectors: {checks} checks passed");
}
