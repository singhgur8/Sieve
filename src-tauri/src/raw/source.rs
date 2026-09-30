//! Random-access byte sources for container parsing.
//!
//! RAW files are 20-60 MB but ingest needs only a few hundred KB of metadata and one
//! embedded JPEG, so parsers read ranges on demand instead of loading the file.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

/// Positional reads over some bytes (a file or an in-memory buffer).
pub trait ByteSource {
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `buf` from `offset`; errors if the range is out of bounds.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;

    fn read_vec(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        check_range(self.len(), offset, len as u64)?;
        let mut v = vec![0u8; len];
        self.read_at(offset, &mut v)?;
        Ok(v)
    }
}

fn check_range(total: u64, offset: u64, len: u64) -> io::Result<()> {
    match offset.checked_add(len) {
        Some(end) if end <= total => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("range {offset}+{len} is outside the {total}-byte source"),
        )),
    }
}

impl ByteSource for [u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        check_range(ByteSource::len(self), offset, buf.len() as u64)?;
        let start = offset as usize;
        buf.copy_from_slice(&self[start..start + buf.len()]);
        Ok(())
    }
}

impl ByteSource for Vec<u8> {
    fn len(&self) -> u64 {
        self.as_slice().len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.as_slice().read_at(offset, buf)
    }
}

/// A file with its first `HEAD` bytes cached: TIFF/RAF/CR3 metadata almost always lives
/// at the start, so most small reads are served from memory; the rest use `pread`.
pub struct FileSource {
    file: File,
    len: u64,
    head: Vec<u8>,
}

impl FileSource {
    const HEAD: usize = 128 * 1024;

    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        let head_len = (len as usize).min(Self::HEAD);
        let mut head = vec![0u8; head_len];
        file.read_exact_at(&mut head, 0)?;
        Ok(Self { file, len, head })
    }

    /// The cached first bytes of the file.
    pub fn head(&self) -> &[u8] {
        &self.head
    }
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        check_range(self.len, offset, buf.len() as u64)?;
        let end = offset + buf.len() as u64;
        if end <= self.head.len() as u64 {
            let start = offset as usize;
            buf.copy_from_slice(&self.head[start..start + buf.len()]);
            Ok(())
        } else {
            self.file.read_exact_at(buf, offset)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_and_file_sources_agree() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, &data).unwrap();
        let file = FileSource::open(&path).unwrap();
        assert_eq!(ByteSource::len(&file), data.len() as u64);
        // Inside the head, straddling it, and past it.
        for (off, len) in [(0u64, 16usize), (131_000, 2_000), (200_000, 5_000)] {
            assert_eq!(file.read_vec(off, len).unwrap(), data.read_vec(off, len).unwrap());
        }
        assert!(file.read_vec(299_999, 2).is_err());
        assert!(data.read_vec(u64::MAX, 2).is_err());
    }
}
