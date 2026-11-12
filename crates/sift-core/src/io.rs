//! Random-access byte source. Parsers only ever pull the few kilobytes of
//! container structure they need, so a 60 MB RAW costs a handful of small
//! positioned reads instead of one big sequential read.

use std::fs::File;
use std::io;
use std::path::Path;

use std::sync::Mutex;

/// Reads are served from whole blocks of this size. Container walks hop
/// between many small structures that sit close together, so one block
/// usually answers dozens of reads; on a network share that turns dozens of
/// round trips into one.
const BLOCK: usize = 64 * 1024;
const MAX_BLOCKS: usize = 12;

pub enum Src {
    File { f: File, len: u64, blocks: Mutex<Vec<(u64, Vec<u8>)>> },
    Mem(Vec<u8>),
}

#[cfg(unix)]
fn pread(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    f.read_at(buf, off)
}

#[cfg(windows)]
fn pread(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    f.seek_read(buf, off)
}

/// Reads up to `buf.len()` bytes at `off`; returns how many were read.
fn read_full(f: &File, buf: &mut [u8], off: u64) -> usize {
    let mut got = 0;
    while got < buf.len() {
        match pread(f, &mut buf[got..], off + got as u64) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    got
}

impl Src {
    pub fn open(path: &Path) -> io::Result<Src> {
        let f = File::open(path)?;
        let len = f.metadata()?.len();
        Ok(Src::File { f, len, blocks: Mutex::new(Vec::new()) })
    }

    pub fn mem(data: Vec<u8>) -> Src {
        Src::Mem(data)
    }

    pub fn len(&self) -> u64 {
        match self {
            Src::File { len, .. } => *len,
            Src::Mem(d) => d.len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reads up to `n` bytes at `off`; the result is shorter at end of file.
    pub fn read_upto(&self, off: u64, n: usize) -> Vec<u8> {
        let total = self.len();
        if off >= total {
            return Vec::new();
        }
        let n = n.min((total - off) as usize);
        match self {
            Src::Mem(d) => d[off as usize..off as usize + n].to_vec(),
            Src::File { f, blocks, .. } => {
                // Large reads (preview payloads) go straight to the file.
                if n > BLOCK {
                    let mut buf = vec![0u8; n];
                    let got = read_full(f, &mut buf, off);
                    buf.truncate(got);
                    return buf;
                }
                let mut out = Vec::with_capacity(n);
                let mut cache = blocks.lock().unwrap();
                let mut pos = off;
                while out.len() < n {
                    let index = pos / BLOCK as u64;
                    let hit = cache.iter().position(|(i, _)| *i == index);
                    let slot = match hit {
                        Some(i) => i,
                        None => {
                            let mut block = vec![0u8; BLOCK.min((total - index * BLOCK as u64) as usize)];
                            let got = read_full(f, &mut block, index * BLOCK as u64);
                            block.truncate(got);
                            if cache.len() >= MAX_BLOCKS {
                                cache.remove(0);
                            }
                            cache.push((index, block));
                            cache.len() - 1
                        }
                    };
                    let block = &cache[slot].1;
                    let start = (pos - index * BLOCK as u64) as usize;
                    if start >= block.len() {
                        break;
                    }
                    let take = (n - out.len()).min(block.len() - start);
                    out.extend_from_slice(&block[start..start + take]);
                    pos += take as u64;
                }
                out
            }
        }
    }

    /// Reads exactly `n` bytes at `off`, or `None` if the range runs past the end.
    pub fn read(&self, off: u64, n: usize) -> Option<Vec<u8>> {
        if off.checked_add(n as u64)? > self.len() {
            return None;
        }
        let v = self.read_upto(off, n);
        (v.len() == n).then_some(v)
    }
}

/// Flushes a file to stable storage. Network shares (SMB, NFS) reject the
/// full flush macOS performs; the bytes are already written by then, so an
/// unsupported flush is not treated as a failed write.
pub fn sync(f: &File) -> io::Result<()> {
    const NOT_SUPPORTED: [i32; 4] = [45, 95, 22, 25]; // ENOTSUP (macOS, Linux), EINVAL, ENOTTY
    match f.sync_all() {
        Err(e) if e.kind() == io::ErrorKind::Unsupported || e.raw_os_error().is_some_and(|c| NOT_SUPPORTED.contains(&c)) => Ok(()),
        other => other,
    }
}

pub fn u16_at(b: &[u8], o: usize, le: bool) -> Option<u16> {
    let s: [u8; 2] = b.get(o..o + 2)?.try_into().ok()?;
    Some(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
}

pub fn u32_at(b: &[u8], o: usize, le: bool) -> Option<u32> {
    let s: [u8; 4] = b.get(o..o + 4)?.try_into().ok()?;
    Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
}

pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}
