//! Header-controlled V5 map sizes must produce controlled errors, never an
//! allocation abort or arithmetic panic (third_party/chd-0.3.4/PATCH.md,
//! "Additional patch: V5 map bounds").
//!
//! Every malformed case runs in a child process (this test binary re-executed
//! under `ulimit -v`), so a regression is a failed assertion about the child's
//! exit status, not a crashed harness, and the address-space cap turns any
//! multi-GiB request into an immediate failure instead of host memory pressure.
use chd::{Chd, Error};
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// A second, allocator-level firewall: even a regression must not actually
// allocate a giant buffer. Only isolated child decoder calls enable tracking.
struct AllocationFirewall;
static TRACK_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static MAX_REQUEST: AtomicUsize = AtomicUsize::new(0);
const DENY_AT: usize = 32 * 1024 * 1024;
fn denied(size: usize) -> bool {
    if TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
        MAX_REQUEST.fetch_max(size, Ordering::Relaxed);
        size >= DENY_AT
    } else {
        false
    }
}
// SAFETY: permitted operations delegate unchanged to System; denied allocation
// requests return null without touching memory. Deallocation always delegates.
unsafe impl GlobalAlloc for AllocationFirewall {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if denied(layout.size()) {
            std::ptr::null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if denied(layout.size()) {
            std::ptr::null_mut()
        } else {
            unsafe { System.alloc_zeroed(layout) }
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if denied(size) {
            std::ptr::null_mut()
        } else {
            unsafe { System.realloc(ptr, layout, size) }
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: AllocationFirewall = AllocationFirewall;

/// A tiny prefix plus virtual zero-filled file space; no large file or Vec.
struct SparseStream {
    prefix: Vec<u8>,
    length: u64,
    position: u64,
}
impl Read for SparseStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = (self.length.saturating_sub(self.position)).min(buf.len() as u64) as usize;
        buf[..count].fill(0);
        if self.position < self.prefix.len() as u64 {
            let start = self.position as usize;
            let copied = count.min(self.prefix.len() - start);
            buf[..copied].copy_from_slice(&self.prefix[start..start + copied]);
        }
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for SparseStream {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.position = match from {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.length.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.position)
    }
}

const MIB: u32 = 1024 * 1024;

fn header(codec: &[u8; 4], logical: u64, hunk: u32, unit: u32, map_offset: u64) -> Vec<u8> {
    let mut b = vec![0u8; 124];
    b[0..8].copy_from_slice(b"MComprHD");
    b[8..12].copy_from_slice(&124u32.to_be_bytes());
    b[12..16].copy_from_slice(&5u32.to_be_bytes());
    b[16..20].copy_from_slice(codec);
    b[32..40].copy_from_slice(&logical.to_be_bytes());
    b[40..48].copy_from_slice(&map_offset.to_be_bytes());
    b[56..60].copy_from_slice(&hunk.to_be_bytes());
    b[60..64].copy_from_slice(&unit.to_be_bytes());
    b
}

/// MSB-first bit packer matching the decoder's `BitReader`.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}
impl Bits {
    fn put(&mut self, value: u64, count: u32) {
        for i in (0..count).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let bit = ((value >> i) & 1) as u8;
            *self.bytes.last_mut().unwrap() |= bit << (7 - self.used % 8);
            self.used += 1;
        }
    }
}

/// A compressed V5 map for one hunk: a two-symbol Huffman tree (symbols 0 and
/// 1, one bit each) then entry 0 = type 0 with a `length_bits`-wide length.
fn one_entry_compressed_map(length_bits: u8, length: u32) -> Vec<u8> {
    let mut bits = Bits::default();
    for symbol in 0..16 {
        if symbol < 2 {
            bits.put(1, 4); // escape ...
            bits.put(1, 4); // ... length 1
        } else {
            bits.put(0, 4); // unused
        }
    }
    bits.put(0, 1); // entry 0: symbol 0 = CompressionType0
    bits.put(u64::from(length), u32::from(length_bits));
    bits.put(0, 16); // crc
    let mut out = Vec::new();
    out.extend_from_slice(&(bits.bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0u8; 6]); // first_offs
    out.extend_from_slice(&0u16.to_be_bytes()); // map_crc
    out.extend_from_slice(&[length_bits, 0, 0, 0]); // + reserved byte: 16-byte preamble
    out.extend_from_slice(&bits.bytes);
    out
}

fn build(case: &str) -> Vec<u8> {
    match case {
        // 4,294,967,295 hunks of 1 MiB; compressed map would be ~48 GiB.
        "count_u32_max_compressed" => header(b"zstd", (1 << 52) - (1 << 20), MIB, MIB, 124),
        // ... and uncompressed (~16 GiB).
        "count_u32_max_uncompressed" => header(&[0; 4], (1 << 52) - (1 << 20), MIB, MIB, 124),
        // Passes optical_preservation's own geometry predicate (~24 GiB).
        "emuwiz_cd_geometry" => header(b"cdlz", 2_147_483_644 * 2448, 2448, 2448, 124),
        // Exactly 2^32 hunks: used to truncate to 0 hunks.
        "count_exactly_2_pow_32" => header(b"zstd", 1 << 52, MIB, MIB, 124),
        "logical_u64_max" => header(b"zstd", u64::MAX, MIB, 2048, 124),
        "logical_u64_max_small_hunk" => header(&[0; 4], u64::MAX, 1, 1, 124),
        // 4 GiB compressed map declared in a tiny file.
        "map_bytes_4gib" => {
            let mut b = header(b"zstd", u64::from(MIB), MIB, 2048, 124);
            b.extend_from_slice(&u32::MAX.to_be_bytes());
            b.extend_from_slice(&[0u8; 12]);
            b
        }
        "map_bytes_in_large_sparse_stream" => build("map_bytes_4gib"),
        // Uncompressed map declared larger than the file.
        "uncompressed_map_truncated" => header(&[0; 4], 1000 * 2048, 2048, 2048, 124),
        "map_offset_u64_max_uncompressed" => header(&[0; 4], 2048, 2048, 2048, u64::MAX),
        "map_offset_wraps_preamble" => header(b"zstd", 2048, 2048, 2048, u64::MAX - 8),
        "map_offset_past_eof_compressed" => header(b"zstd", 2048, 2048, 2048, 1 << 40),
        "uncompressed_map_inside_header" => header(&[0; 4], 2048, 2048, 2048, 0),
        // Entry length wider than the packed 24-bit field.
        "entry_length_over_24_bits" => {
            let mut b = header(b"zstd", 2048, 2048, 2048, 124);
            b.extend_from_slice(&one_entry_compressed_map(32, 0x0100_0000));
            b
        }
        // Control: the same builder with an in-range length is parsed to the
        // CRC check (a controlled error here), proving the crafted stream is
        // well formed and the case above is refused for its length.
        "entry_length_in_range" => {
            let mut b = header(b"zstd", 2048, 2048, 2048, 124);
            b.extend_from_slice(&one_entry_compressed_map(32, 0x00FF_FFFF));
            b
        }
        other => panic!("unknown case {other}"),
    }
}

const CASES: &[(&str, &str)] = &[
    ("count_u32_max_compressed", "OutOfMemory"),
    ("count_u32_max_uncompressed", "OutOfMemory"),
    ("emuwiz_cd_geometry", "OutOfMemory"),
    ("count_exactly_2_pow_32", "InvalidData"),
    ("logical_u64_max", "InvalidData"),
    ("logical_u64_max_small_hunk", "InvalidData"),
    ("map_bytes_4gib", "InvalidFile"),
    ("map_bytes_in_large_sparse_stream", "OutOfMemory"),
    ("uncompressed_map_truncated", "InvalidFile"),
    ("map_offset_u64_max_uncompressed", "InvalidFile"),
    ("map_offset_wraps_preamble", "InvalidFile"),
    ("map_offset_past_eof_compressed", "InvalidFile"),
    ("uncompressed_map_inside_header", "InvalidFile"),
    ("entry_length_over_24_bits", "DecompressionError"),
    ("entry_length_in_range", "DecompressionError"),
];

/// Child body: runs only when spawned by `malformed_headers_fail_in_bounded_child`.
#[test]
#[ignore]
fn child_open_case() {
    let Ok(case) = std::env::var("CHD_MAP_CASE") else {
        return;
    };
    let bytes = build(&case);
    MAX_REQUEST.store(0, Ordering::Relaxed);
    TRACK_ALLOCATIONS.store(true, Ordering::Relaxed);
    let result = if case == "map_bytes_in_large_sparse_stream" {
        Chd::open(
            SparseStream {
                prefix: bytes,
                length: (1u64 << 32) + 4096,
                position: 0,
            },
            None,
        )
        .map(|_| ())
    } else {
        Chd::open(Cursor::new(bytes), None).map(|_| ())
    };
    TRACK_ALLOCATIONS.store(false, Ordering::Relaxed);
    let largest = MAX_REQUEST.load(Ordering::Relaxed);
    assert!(
        largest < DENY_AT,
        "decoder attempted oversized allocation: {largest}"
    );
    println!("MAX_ALLOCATION {largest}");
    match result {
        Ok(_) => println!("RESULT Ok"),
        Err(e) => println!("RESULT {e:?}"),
    }
}

#[test]
fn malformed_headers_fail_in_bounded_child() {
    let exe = std::env::current_exe().unwrap();
    let mut failures = Vec::new();
    for (case, expected) in CASES {
        // 512 MiB of address space: far below every request above, far above
        // what the test harness itself needs.
        let output = Command::new("sh")
            .arg("-c")
            .arg("ulimit -c 0; ulimit -v 524288; exec \"$0\" --ignored --exact child_open_case --nocapture")
            .arg(&exe)
            .env("CHD_MAP_CASE", case)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!(
            "{case}: {}",
            stdout
                .lines()
                .filter(|line| line.starts_with("MAX_ALLOCATION ") || line.starts_with("RESULT "))
                .collect::<Vec<_>>()
                .join("; ")
        );
        if !output.status.success() || !stdout.contains(&format!("RESULT {expected}")) {
            failures.push(format!(
                "{case}: expected {expected}; child {:?}\n{stdout}\n{stderr}",
                output.status
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn truncation_is_an_error_at_every_prefix_of_a_valid_compressed_map() {
    // A well-formed map (control case above reaches the CRC check) cut at every
    // length must never panic or request memory from the header.
    let full = {
        let mut b = header(b"zstd", 2048, 2048, 2048, 124);
        b.extend_from_slice(&one_entry_compressed_map(8, 7));
        b
    };
    for cut in 0..full.len() {
        let r = Chd::open(Cursor::new(full[..cut].to_vec()), None);
        assert!(r.is_err(), "prefix {cut} unexpectedly opened");
    }
    assert!(matches!(
        Chd::open(Cursor::new(full), None),
        Err(Error::DecompressionError)
    ));
}
