// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Container-level metadata: footer, CMO/GBO tables, column metadata.
//!
//! Everything here comes from the data file itself. No dataset manifest is
//! required; the only mapping that cannot be recovered here is field id ->
//! column index, which lives in the manifest's `DataFile` entry.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use prost::Message;

use crate::format::footer::{FOOTER_LEN_V1, FOOTER_LEN_V2, Footer, Region, V2Footer, Version};
use crate::proto::{pb, pb2};
use crate::source::FileSource;

/// A byte range in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferRange {
  pub position: u64,
  pub size: u64,
}

impl BufferRange {
  pub fn end(&self) -> u64 {
    self.position.saturating_add(self.size)
  }
}

/// Page-level metadata used by `info` now, and by `pages` / `page` later.
#[derive(Debug)]
#[allow(dead_code)]
pub struct PageMeta {
  pub index: u32,
  pub buffers: Vec<BufferRange>,
  /// `Page.length`: the top-level row count of the page.
  pub num_rows: u64,
  /// `Page.priority`: the row number of the first row in the page.
  pub priority: u64,
  /// Raw page encoding. Decoded by later phases (`pages` / `page`).
  #[allow(dead_code)]
  pub encoding: Option<pb2::Encoding>,
}

#[derive(Debug)]
pub struct ColumnMeta {
  pub index: u32,
  pub pages: Vec<PageMeta>,
  /// Column-level buffers (dictionaries/statistics in v2.0).
  pub column_buffers: Vec<BufferRange>,
  /// True when the column metadata record could not be decoded.
  pub undecodable: bool,
}

impl ColumnMeta {
  fn undecodable(index: u32) -> Self {
    Self {
      index,
      pages: Vec::new(),
      column_buffers: Vec::new(),
      undecodable: true,
    }
  }

  pub fn total_rows(&self) -> u64 {
    self.pages.iter().map(|page| page.num_rows).sum()
  }

  /// Sum of page and column buffer sizes. Excludes 64-byte alignment padding.
  pub fn total_bytes(&self) -> u64 {
    self
      .pages
      .iter()
      .flat_map(|page| page.buffers.iter())
      .map(|buffer| buffer.size)
      .sum::<u64>()
      + self
        .column_buffers
        .iter()
        .map(|buffer| buffer.size)
        .sum::<u64>()
  }
}

#[derive(Debug)]
pub struct FileMeta {
  pub path: PathBuf,
  pub file_size: u64,
  pub footer: Footer,
  pub version: Version,
  pub global_buffers: Vec<BufferRange>,
  /// CMO entries: where each column's `ColumnMetadata` record lives.
  pub column_metadata_table: Vec<BufferRange>,
  pub schema: Option<pb::Schema>,
  pub num_rows: Option<u64>,
  pub columns: Vec<ColumnMeta>,
  pub warnings: Vec<String>,
}

impl FileMeta {
  pub fn open(path: &std::path::Path) -> Result<Self> {
    let mut source = FileSource::open(path)?;
    Self::open_from(&mut source)
  }

  pub fn open_from(source: &mut FileSource) -> Result<Self> {
    let file_size = source.len();
    let path = source.path().to_path_buf();

    if file_size < FOOTER_LEN_V1 {
      bail!(
        "file is too small to be a Lance file: {file_size} bytes \
                 (the smallest footer is {FOOTER_LEN_V1} bytes)"
      );
    }

    let tail_len = file_size.min(FOOTER_LEN_V2) as usize;
    let tail = source.read_at(file_size - tail_len as u64, tail_len)?;
    let footer = Footer::parse(&tail)?;
    let version = footer.version();
    let mut warnings = Vec::new();

    if !version.is_v2() {
      bail!(
        "unsupported Lance file format {version} (major {}, minor {}): \
         ldp supports v2.0-v2.3 only",
        footer.major_version,
        footer.minor_version
      );
    }

    let v2 = *footer
      .v2()
      .expect("a v2 version always has a v2 footer layout");

    validate_regions(&v2, file_size, &mut warnings);

    let global_buffers = read_offset_table(
      source,
      v2.global_buff_offsets_start,
      v2.num_global_buffers,
      "GBO",
    )?;
    let column_metadata_table =
      read_offset_table(source, v2.column_meta_offsets_start, v2.num_columns, "CMO")?;

    if global_buffers.is_empty() {
      bail!("file contains no global buffers; global buffer 0 is expected to hold the file schema");
    }

    let schema_buffer = global_buffers[0];
    if schema_buffer.end() > file_size {
      bail!(
        "global buffer 0 (schema) is out of bounds: {}..{} (file size {file_size})",
        schema_buffer.position,
        schema_buffer.end()
      );
    }
    let schema_bytes = source.read_at(schema_buffer.position, schema_buffer.size as usize)?;
    let descriptor = pb::FileDescriptor::decode(schema_bytes.as_slice())
      .context("failed to decode global buffer 0 as FileDescriptor")?;
    let schema = descriptor.schema;
    let num_rows = Some(descriptor.length);

    let mut columns = Vec::with_capacity(column_metadata_table.len());
    for (index, entry) in column_metadata_table.iter().enumerate() {
      let index = index as u32;
      if entry.position < v2.column_meta_start || entry.end() > v2.column_meta_offsets_start {
        warnings.push(format!(
          "column {index}: metadata range {}..{} is outside the column metadata region {}..{}",
          entry.position,
          entry.end(),
          v2.column_meta_start,
          v2.column_meta_offsets_start
        ));
      }
      if entry.end() > file_size {
        warnings.push(format!(
          "column {index}: metadata range {}..{} is out of bounds (file size {file_size})",
          entry.position,
          entry.end()
        ));
        columns.push(ColumnMeta::undecodable(index));
        continue;
      }
      match source.read_at(entry.position, entry.size as usize) {
        Ok(bytes) => match pb2::ColumnMetadata::decode(bytes.as_slice()) {
          Ok(metadata) => {
            columns.push(decode_column(index, metadata, &mut warnings));
          }
          Err(error) => {
            warnings.push(format!(
              "column {index}: failed to decode ColumnMetadata: {error}"
            ));
            columns.push(ColumnMeta::undecodable(index));
          }
        },
        Err(error) => {
          warnings.push(format!("column {index}: {error:#}"));
          columns.push(ColumnMeta::undecodable(index));
        }
      }
    }

    Ok(Self {
      path,
      file_size,
      footer,
      version,
      global_buffers,
      column_metadata_table,
      schema,
      num_rows,
      columns,
      warnings,
    })
  }

  /// The five regions of a v2 file. `None` for legacy v1 files, whose footer
  /// has no region offsets.
  pub fn regions(&self) -> Option<Vec<Region>> {
    self.footer.regions(self.file_size)
  }

  /// The schema embedded in global buffer 0.
  pub fn schema(&self) -> Result<&pb::Schema> {
    self.schema.as_ref().ok_or_else(|| {
      anyhow::anyhow!("no schema available: global buffer 0 does not contain a schema")
    })
  }

  pub fn num_columns(&self) -> Option<u32> {
    self.footer.v2().map(|footer| footer.num_columns)
  }

  pub fn num_global_buffers(&self) -> Option<u32> {
    self.footer.v2().map(|footer| footer.num_global_buffers)
  }
}

fn decode_column(
  index: u32,
  metadata: pb2::ColumnMetadata,
  warnings: &mut Vec<String>,
) -> ColumnMeta {
  let pages = metadata
    .pages
    .into_iter()
    .enumerate()
    .map(|(page_index, page)| {
      if page.buffer_offsets.len() != page.buffer_sizes.len() {
        warnings.push(format!(
          "column {index} page {page_index}: {} buffer offset(s) but {} size(s)",
          page.buffer_offsets.len(),
          page.buffer_sizes.len()
        ));
      }
      let buffers = page
        .buffer_offsets
        .iter()
        .copied()
        .zip(page.buffer_sizes.iter().copied())
        .map(|(position, size)| BufferRange { position, size })
        .collect();
      PageMeta {
        index: page_index as u32,
        buffers,
        num_rows: page.length,
        priority: page.priority,
        encoding: page.encoding,
      }
    })
    .collect();

  if metadata.buffer_offsets.len() != metadata.buffer_sizes.len() {
    warnings.push(format!(
      "column {index}: {} column buffer offset(s) but {} size(s)",
      metadata.buffer_offsets.len(),
      metadata.buffer_sizes.len()
    ));
  }
  let column_buffers = metadata
    .buffer_offsets
    .iter()
    .copied()
    .zip(metadata.buffer_sizes.iter().copied())
    .map(|(position, size)| BufferRange { position, size })
    .collect();

  ColumnMeta {
    index,
    pages,
    column_buffers,
    undecodable: false,
  }
}

fn read_offset_table(
  source: &mut FileSource,
  start: u64,
  count: u32,
  label: &str,
) -> Result<Vec<BufferRange>> {
  let byte_len = (count as u64)
    .checked_mul(16)
    .ok_or_else(|| anyhow::anyhow!("{label} table size overflows"))?;
  if start
    .checked_add(byte_len)
    .is_none_or(|end| end > source.len())
  {
    bail!(
      "{label} table is out of bounds: start={start}, entries={count}, file_size={}",
      source.len()
    );
  }
  let bytes = source.read_at(start, byte_len as usize)?;
  let mut entries = Vec::with_capacity(count as usize);
  for chunk in bytes.chunks_exact(16) {
    let position = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
    let size = u64::from_le_bytes(chunk[8..16].try_into().unwrap());
    entries.push(BufferRange { position, size });
  }
  Ok(entries)
}

fn validate_regions(footer: &V2Footer, file_size: u64, warnings: &mut Vec<String>) {
  let footer_start = file_size.saturating_sub(FOOTER_LEN_V2);
  let cmo_end = footer.column_meta_offsets_start + (footer.num_columns as u64).saturating_mul(16);
  let gbo_end =
    footer.global_buff_offsets_start + (footer.num_global_buffers as u64).saturating_mul(16);

  if !(footer.column_meta_start <= footer.column_meta_offsets_start
    && footer.column_meta_offsets_start <= footer.global_buff_offsets_start
    && footer.global_buff_offsets_start <= footer_start)
  {
    warnings.push(format!(
      "footer region offsets are not monotonically increasing: \
             column_meta_start={}, cmo_start={}, gbo_start={}, footer_start={}",
      footer.column_meta_start,
      footer.column_meta_offsets_start,
      footer.global_buff_offsets_start,
      footer_start
    ));
  }
  if cmo_end != footer.global_buff_offsets_start {
    warnings.push(format!(
      "CMO table does not end at GBO start: cmo_end={cmo_end}, gbo_start={}",
      footer.global_buff_offsets_start
    ));
  }
  if gbo_end != footer_start {
    warnings.push(format!(
      "GBO table does not end at the footer: gbo_end={gbo_end}, footer_start={footer_start}"
    ));
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
      .join("tests/fixtures")
      .join(name)
  }

  #[test]
  fn reads_v2_1_container() {
    let meta = FileMeta::open(&fixture("v2_1.lance")).unwrap();
    assert_eq!(meta.version, Version::V2_1);
    assert_eq!(meta.num_rows, Some(256));
    assert_eq!(meta.num_columns(), Some(1));
    assert_eq!(meta.schema.as_ref().unwrap().fields.len(), 1);
    assert_eq!(meta.columns.len(), 1);
    assert_eq!(meta.columns[0].pages.len(), 1);
    assert_eq!(meta.columns[0].total_rows(), 256);
    assert_eq!(meta.columns[0].pages[0].buffers.len(), 2);
    assert!(meta.warnings.is_empty());
  }

  #[test]
  fn reads_v2_2_container() {
    let meta = FileMeta::open(&fixture("v2_2.lance")).unwrap();
    assert_eq!(meta.version, Version::V2_2);
    assert_eq!(meta.num_rows, Some(66_049));
    assert_eq!(meta.num_columns(), Some(2));
    assert_eq!(meta.columns.len(), 2);
    // Column 0 is a constant all-null page with a single buffer.
    assert_eq!(meta.columns[0].pages[0].buffers.len(), 1);
    // Column 1 is a mini-block page with metadata, data, and rep index.
    assert_eq!(meta.columns[1].pages[0].buffers.len(), 3);
  }

  #[test]
  fn reads_v2_0_container() {
    let meta = FileMeta::open(&fixture("v2_0.lance")).unwrap();
    // Footer numbers (0, 3) mean v2.0, not v1.
    assert_eq!(meta.version, Version::V2_0);
    assert_eq!(meta.footer.minor_version, 3);
    assert!(meta.schema.is_some());
  }

  #[test]
  fn legacy_file_is_rejected() {
    let error = FileMeta::open(&fixture("v1.lance"))
      .unwrap_err()
      .to_string();
    assert!(error.contains("unsupported"), "error: {error}");
    assert!(error.contains("v1"), "error: {error}");
  }

  #[test]
  fn info_does_not_read_page_buffers() {
    let path = fixture("v2_2.lance");
    let mut source = FileSource::open(&path).unwrap();
    let meta = FileMeta::open_from(&mut source).unwrap();

    let page_buffers: Vec<BufferRange> = meta
      .columns
      .iter()
      .flat_map(|column| column.pages.iter())
      .flat_map(|page| page.buffers.iter().copied())
      .collect();
    assert!(!page_buffers.is_empty());

    for &(start, len) in source.read_ranges() {
      let end = start + len as u64;
      for buffer in &page_buffers {
        let overlaps = start < buffer.end() && buffer.position < end;
        assert!(
          !overlaps,
          "container loading read page buffer {}..{} (read {}..{})",
          buffer.position,
          buffer.end(),
          start,
          end
        );
      }
    }
  }

  #[test]
  fn missing_file_is_an_error() {
    let error = FileMeta::open(&fixture("does-not-exist.lance"))
      .unwrap_err()
      .to_string();
    assert!(error.contains("failed to open"));
  }
}
