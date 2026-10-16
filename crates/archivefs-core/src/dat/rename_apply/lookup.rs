//! Read-only lookup seam for recovery decisions. Production delegates to std;
//! tests inject local failures without touching mounts or production storage.
use std::path::Path;
use std::{fs, io};

pub(super) fn symlink_metadata(path: &Path) -> io::Result<fs::Metadata> {
    #[cfg(test)]
    if let Some(error) = test_fault(path) {
        return Err(error);
    }
    fs::symlink_metadata(path)
}

pub(super) fn read_dir(path: &Path) -> io::Result<fs::ReadDir> {
    #[cfg(test)]
    if let Some(error) = test_fault(path) {
        return Err(error);
    }
    fs::read_dir(path)
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::RefCell<Option<(std::ffi::OsString, i32)>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn test_fault(path: &Path) -> Option<io::Error> {
    FAULT.with(|fault| {
        fault.borrow().as_ref().and_then(|(name, errno)| {
            (path.file_name() == Some(name.as_os_str()))
                .then(|| io::Error::from_raw_os_error(*errno))
        })
    })
}
#[cfg(test)]
pub(super) struct Fault;
#[cfg(test)]
impl Fault {
    pub(super) fn at(path: &Path, errno: i32) -> Self {
        FAULT.with(|fault| {
            assert!(fault.borrow().is_none());
            *fault.borrow_mut() = Some((path.file_name().unwrap().to_owned(), errno));
        });
        Self
    }
}
#[cfg(test)]
impl Drop for Fault {
    fn drop(&mut self) {
        FAULT.with(|fault| *fault.borrow_mut() = None);
    }
}
