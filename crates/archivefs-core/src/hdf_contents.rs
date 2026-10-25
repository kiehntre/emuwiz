//! Demand-driven Amiga image directory inspection over the existing Amiga reader.
//! Included under `amiga_disk::filesystem` to reuse its bounded device and
//! metadata validation. Container probes may overlap file-data regions;
//! no file-data traversal, extraction, mounts or writes.
use super::super::{inspect_amiga_image_opened, source_identity::OpenedSource};
use super::*;
use affs_read::EntryType;
use std::collections::BTreeMap;
use std::path::Path;

pub const PAGE_ENTRIES: usize = 128;
pub const MAX_DIRECTORY_ENTRIES: usize = 4096;
const MAX_READS: u64 = 8192;
const MAX_DIRECTORIES: usize = 10_000;

/// Only directories yielded by this inspection can be opened. Never a host path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Directory {
    partition: usize,
    block: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: String,
    pub size: Option<u32>,
    pub directory: Option<Directory>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryPage {
    pub filesystem: AmigaFilesystem,
    pub entries: Vec<Entry>,
    pub next_offset: Option<usize>,
    /// A bounded prefix is not a complete directory or an integrity verdict.
    pub limitation: Option<String>,
}

/// Descriptor-pinned, read-only session. All directory reads stay inside the
/// selected partition and use the same image object as container inspection.
pub struct Inspection {
    disk: AmigaDisk,
    file: File,
    source: OpenedSource,
    directories: BTreeMap<Directory, usize>,
}

impl Inspection {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file =
            crate::safe_read::open_bounded_read(path, &crate::safe_read::TrustedRoots::none())
                .map_err(|e| format!("Unsafe or unavailable Amiga filesystem image: {e:?}"))?
                .into_file();
        let source = OpenedSource::capture(&file.metadata().map_err(|e| e.to_string())?);
        let disk = inspect_amiga_image_opened(path, &file).map_err(|e| e.to_string())?;
        validate_layout(&disk, &file)?;
        let inspection = Self {
            disk,
            file,
            source,
            directories: BTreeMap::new(),
        };
        inspection.ensure_current()?;
        Ok(inspection)
    }

    pub fn disk(&self) -> &AmigaDisk {
        &self.disk
    }

    pub fn is_current(&self) -> bool {
        self.source
            .unchanged(&self.file, &self.disk.path)
            .unwrap_or(false)
    }

    fn ensure_current(&self) -> Result<(), String> {
        self.is_current().then_some(()).ok_or_else(||
            "Image changed or became unavailable. Inspect again; old contents are no longer current.".into())
    }

    fn device(&self, partition: &Partition) -> Result<PartitionRangeDevice, String> {
        Ok(PartitionRangeDevice {
            file: self.file.try_clone().map_err(|e| e.to_string())?,
            offset: partition.byte_offset,
            length: partition.byte_length,
            max_reads: MAX_READS,
            reads: AtomicU64::new(0),
            io_failure: Mutex::new(None),
        })
    }

    pub fn root(&mut self, partition: usize) -> Result<Directory, String> {
        self.ensure_current()?;
        let part = self
            .disk
            .rdb
            .partitions
            .get(partition)
            .ok_or("Partition is not in this inspection")?;
        let device = self.device(part)?;
        reader_metadata(&device, part).map_err(|e| e.to_string())?;
        let reader = AffsReader::with_size(&device, (part.byte_length / SECTOR_BYTES) as u32)
            .map_err(|e| format!("Filesystem root rejected: {e:?}"))?;
        let directory = Directory {
            partition,
            block: reader.root_block(),
        };
        self.ensure_current()?;
        if !self.directories.contains_key(&directory) && self.directories.len() >= MAX_DIRECTORIES {
            return Err(
                "Directory-node limit reached; refresh before inspecting another partition.".into(),
            );
        }
        self.directories.insert(directory, 0);
        Ok(directory)
    }

    /// Read one bounded page of a single requested directory. No recursion and
    /// no reads of file data. Page offsets replay at most 4096 metadata entries.
    pub fn list(&mut self, directory: Directory, offset: usize) -> Result<DirectoryPage, String> {
        self.ensure_current()?;
        let depth = *self
            .directories
            .get(&directory)
            .ok_or("Directory was not observed in this inspection")?;
        if offset >= MAX_DIRECTORY_ENTRIES {
            return Err("Directory listing limit reached".into());
        }
        let part = &self.disk.rdb.partitions[directory.partition];
        let device = self.device(part)?;
        let filesystem = reader_metadata(&device, part).map_err(|e| e.to_string())?;
        let checked = DirectoryDevice(&device);
        let reader = AffsReader::with_size(&checked, (part.byte_length / SECTOR_BYTES) as u32)
            .map_err(|e| format!("Filesystem rejected: {e:?}"))?;
        let iter = reader
            .read_dir(directory.block)
            .map_err(|e| format!("Directory rejected: {e:?}"))?;
        let mut seen = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut entries = Vec::new();
        let mut discovered = Vec::new();
        let mut next_offset = None;
        let mut limitation = None;
        for (index, entry) in iter.enumerate() {
            if index >= MAX_DIRECTORY_ENTRIES {
                limitation = Some(format!(
                    "Directory limited to {MAX_DIRECTORY_ENTRIES} entries; more may exist."
                ));
                break;
            }
            let entry =
                entry.map_err(|e| format!("Directory listing refused (unsupported/corrupt metadata, unavailable data, or read budget exhausted): {e:?}"))?;
            if !seen.insert(entry.block)
                || !names.insert(
                    entry
                        .name()
                        .iter()
                        .map(|&byte| {
                            if filesystem.international {
                                affs_read::intl_to_upper(byte)
                            } else {
                                byte.to_ascii_uppercase()
                            }
                        })
                        .collect::<Vec<_>>(),
                )
                || entry.parent != directory.block
                || entry.block == directory.block
            {
                return Err("Ambiguous or cyclic directory structure; listing refused.".into());
            }
            if index < offset {
                continue;
            }
            if entries.len() == PAGE_ENTRIES {
                next_offset = Some(index);
                break;
            }
            let child = Directory {
                partition: directory.partition,
                block: entry.block,
            };
            let is_directory = entry.entry_type == EntryType::Dir;
            let browsable = is_directory
                && depth < 32
                && self.directories.len() + discovered.len() < MAX_DIRECTORIES;
            if browsable {
                discovered.push((child, depth + 1));
            }
            entries.push(Entry {
                name: display_component(entry.name()),
                kind: match entry.entry_type {
                    EntryType::Dir if !browsable => {
                        "Directory (inspection depth/node limit)".into()
                    }
                    EntryType::Dir => "Directory".into(),
                    EntryType::File => "File (header size; data not checked)".into(),
                    other => format!("{other:?} (link/special entry; not followed)"),
                },
                size: (entry.entry_type == EntryType::File).then_some(entry.size),
                directory: browsable.then_some(child),
            });
        }
        self.ensure_current()?;
        self.directories.extend(discovered);
        Ok(DirectoryPage {
            filesystem,
            entries,
            next_offset,
            limitation,
        })
    }
}

// Upstream DirIter silently skips unrecognized secondary entry types. Use
// its own parser to refuse those structures rather than show an empty listing.
struct DirectoryDevice<'a>(&'a PartitionRangeDevice);
impl BlockDevice for DirectoryDevice<'_> {
    fn read_block(&self, block: u32, bytes: &mut [u8; 512]) -> Result<(), ()> {
        self.0.read_block(block, bytes)?;
        if u32::from_be_bytes(bytes[..4].try_into().unwrap()) == 2 {
            let entry = affs_read::EntryBlock::parse(bytes).map_err(|_| ())?;
            if entry.entry_type().is_none()
                || bytes[0x1B0] as usize > affs_read::MAX_NAME_LEN
                || (entry.entry_type() != Some(EntryType::Root) && bytes[0x1B0] == 0)
            {
                return Err(());
            }
        }
        Ok(())
    }
}

fn validate_layout(disk: &AmigaDisk, file: &File) -> Result<(), String> {
    if disk.rdb.partitions.is_empty() {
        return Err("No partitions were evidenced; no filesystem contents can be listed.".into());
    }
    let mut ranges = Vec::new();
    let flat = disk.rdb.partitions.len() == 1
        && disk.rdb.partitions[0].byte_offset == 0
        && disk.rdb.partition_head == u32::MAX;
    for partition in &disk.rdb.partitions {
        let end = partition
            .byte_offset
            .checked_add(partition.byte_length)
            .ok_or("Partition overflow")?;
        if partition.byte_length < 1024
            || partition.byte_length % 512 != 0
            || partition.byte_length / 512 > u64::from(u32::MAX)
            || end > disk.image_size
            || ranges
                .iter()
                .any(|&(start, stop)| partition.byte_offset < stop && start < end)
        {
            return Err("Unsafe or overlapping partition ranges; inspection refused.".into());
        }
        if !flat && partition.boot_signature != Some(partition.dos_type) {
            return Err(
                "Partition filesystem type conflicts with its boot signature; inspection refused."
                    .into(),
            );
        }
        ranges.push((partition.byte_offset, end));
    }
    if !flat {
        // The container parser detects metadata; this consumer additionally
        // refuses damaged RDB/PART checksums before presenting contents.
        let mut blocks = vec![(disk.rdb.block_index, 104_usize)];
        blocks.extend(
            disk.rdb
                .partitions
                .iter()
                .map(|p| (u64::from(p.block_index), 196)),
        );
        for (index, minimum) in blocks {
            let mut block = vec![0; disk.rdb.block_size as usize];
            let offset = index
                .checked_mul(u64::from(disk.rdb.block_size))
                .ok_or("Metadata offset overflow")?;
            if ranges
                .iter()
                .any(|&(start, end)| offset < end && start < offset + block.len() as u64)
            {
                return Err("Partition overlaps container metadata; inspection refused.".into());
            }
            read_exact_at(file, offset, &mut block).map_err(|e| e.to_string())?;
            let words = u32::from_be_bytes(block[4..8].try_into().unwrap()) as usize;
            if words < minimum.div_ceil(4)
                || words > block.len() / 4
                || block[..words * 4].chunks_exact(4).fold(0_u32, |sum, word| {
                    sum.wrapping_add(u32::from_be_bytes(word.try_into().unwrap()))
                }) != 0
            {
                return Err(
                    "RDB/PART checksum or structural length is invalid; inspection refused.".into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "hdf_contents_tests.rs"]
mod tests;

/// Thread-local test instrumentation only. It observes the existing read sites;
/// production uses the same descriptors, dispatch, offsets and budgets unchanged.
#[cfg(test)]
pub(crate) mod read_audit {
    use std::cell::RefCell;
    #[derive(Default, Debug)]
    pub struct Trace {
        pub container: Vec<(u64, usize)>,
        pub filesystem: Vec<(u64, usize)>,
    }
    thread_local! { static ACTIVE: RefCell<Option<Trace>> = const { RefCell::new(None) }; }
    pub fn container(offset: u64, bytes: usize) {
        ACTIVE.with(|active| {
            if let Some(trace) = active.borrow_mut().as_mut() {
                trace.container.push((offset, bytes));
            }
        });
    }
    pub fn filesystem(offset: u64, bytes: usize) {
        ACTIVE.with(|active| {
            if let Some(trace) = active.borrow_mut().as_mut() {
                trace.filesystem.push((offset, bytes));
            }
        });
    }
    pub fn capture<T>(run: impl FnOnce() -> T) -> (T, Trace) {
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                ACTIVE.with(|active| {
                    active.borrow_mut().take();
                });
            }
        }
        ACTIVE.with(|active| {
            assert!(active.borrow().is_none());
            *active.borrow_mut() = Some(Trace::default());
        });
        let _clear = Clear;
        let result = run();
        let trace = ACTIVE.with(|active| active.borrow_mut().take().unwrap());
        (result, trace)
    }
}
