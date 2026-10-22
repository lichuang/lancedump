// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Random-access reads over a local file.
//!
//! `ldp` is a single-threaded, read-only CLI, so `seek` + `read_exact` is
//! enough and keeps the tool free of async I/O and object-store dependencies.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub struct FileSource {
  path: PathBuf,
  file: File,
  len: u64,
  read_ranges: Vec<(u64, usize)>,
}

impl FileSource {
  pub fn open(path: &Path) -> Result<Self> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let len = file
      .metadata()
      .with_context(|| format!("failed to stat {}", path.display()))?
      .len();
    Ok(Self {
      path: path.to_path_buf(),
      file,
      len,
      read_ranges: Vec::new(),
    })
  }

  pub fn path(&self) -> &Path {
    &self.path
  }

  pub fn len(&self) -> u64 {
    self.len
  }

  /// Every range read so far, in call order. Used by tests to assert that a
  /// command only touched the metadata it claims to touch.
  #[cfg(test)]
  pub fn read_ranges(&self) -> &[(u64, usize)] {
    &self.read_ranges
  }

  pub fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
    let end = offset
      .checked_add(len as u64)
      .ok_or_else(|| anyhow::anyhow!("read range overflows: offset={offset}, len={len}"))?;
    if end > self.len {
      bail!(
        "read out of bounds: offset={offset}, len={len}, file_size={}",
        self.len
      );
    }
    self.read_ranges.push((offset, len));
    self.file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    self
      .file
      .read_exact(&mut buf)
      .with_context(|| format!("failed to read {len} bytes at offset {offset}"))?;
    Ok(buf)
  }
}
