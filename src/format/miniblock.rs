// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Decoding of the page-internal structure of `MiniBlockLayout` pages.
//!
//! A mini-block page has (usually) two page buffers:
//!
//! ```text
//! buffer 0: chunk metadata, one entry per chunk:
//!           u16 (v2.1) or u32 (v2.2+):
//!             bits [4..] : chunk_size / 8 - 1
//!             bits [0..4]: log2(number of values); a partial final chunk
//!                          writes 0, a full final chunk may carry the real log
//! buffer 1: chunk data, 8-byte aligned chunks concatenated
//! buffer 2: dictionary (optional)
//! buffer 2/3: repetition index (optional, depth + 1 u64 per chunk)
//! ```
//!
//! Each chunk is self-contained:
//!
//! ```text
//! u16 num_levels
//! u16 rep_size     (only when rep levels exist; always u16)
//! u16 def_size     (only when def levels exist; always u16)
//! u16/u32 value_buffer_size[num_buffers]
//! padding to 8 bytes
//! rep bytes, padded to 8
//! def bytes, padded to 8
//! value buffer 0..N-1, each padded to 8
//! ```

use anyhow::{Result, bail};

use crate::format::container::PageMeta;
use crate::proto::pb21;

/// One decoded mini-block metadata entry (one chunk).
#[derive(Debug, Clone, Copy)]
pub struct ChunkMetadata {
  /// Raw metadata word (u16 or u32 widened to u64).
  pub word: u64,
  /// Number of values in the chunk. The final chunk is resolved from
  /// `layout.num_items`; everything else is derived from `word`.
  pub values: u64,
}

impl ChunkMetadata {
  /// Parse one metadata word.
  ///
  /// `seen_values` is the number of values accumulated by all previous chunks;
  /// the final chunk resolves its count from `num_items` instead of the
  /// (possibly zero) `log2` field.
  pub fn parse(
    index: usize,
    is_final: bool,
    word: u64,
    num_items: u64,
    seen_values: u64,
  ) -> Result<Self> {
    let log2_values = (word & 0xF) as u8;
    let values = if is_final {
      let Some(values) = num_items.checked_sub(seen_values) else {
        bail!("non-final chunks account for {seen_values} values, exceeding num_items {num_items}");
      };
      values
    } else {
      if log2_values == 0 {
        bail!("non-final chunk {index} has log_num_values=0");
      }
      1u64 << log2_values
    };
    if is_final && log2_values != 0 {
      let encoded = 1u64 << log2_values;
      if encoded != values {
        bail!(
          "final chunk log_num_values={log2_values} implies {encoded} values but {values} remain"
        );
      }
    }

    Ok(Self { word, values })
  }

  /// Total chunk size in bytes (8-byte aligned).
  pub fn bytes(&self) -> u64 {
    ((self.word >> 4) + 1) * 8
  }

  /// Raw `log2(number of values)` field. A partial final chunk stores 0; a
  /// full final chunk may store its real log. Use [`Self::values`] for the
  /// resolved count.
  pub fn log2_values(&self) -> u8 {
    (self.word & 0xF) as u8
  }
}

/// The chunk metadata buffer, decoded into one entry per chunk.
#[derive(Debug)]
pub struct ChunkMetadataTable {
  /// 2 bytes for v2.1 (`u16`), 4 bytes for v2.2+ (`u32`).
  pub entry_width: usize,
  pub entries: Vec<ChunkMetadata>,
}

impl ChunkMetadataTable {
  pub fn parse(metadata: &[u8], layout: &pb21::MiniBlockLayout) -> Result<Self> {
    let entry_width = if layout.has_large_chunk { 4 } else { 2 };
    if !metadata.len().is_multiple_of(entry_width) {
      bail!(
        "mini-block metadata buffer is {} bytes, not a multiple of the {entry_width}-byte entry",
        metadata.len()
      );
    }
    let num_chunks = metadata.len() / entry_width;
    if num_chunks == 0 {
      bail!("mini-block page has an empty metadata buffer (no chunks)");
    }

    let mut entries = Vec::with_capacity(num_chunks);
    let mut seen_values = 0u64;
    for (index, raw) in metadata.chunks_exact(entry_width).enumerate() {
      let word = if entry_width == 4 {
        u32::from_le_bytes(raw.try_into().unwrap()) as u64
      } else {
        u16::from_le_bytes(raw.try_into().unwrap()) as u64
      };
      let entry = ChunkMetadata::parse(
        index,
        index + 1 == num_chunks,
        word,
        layout.num_items,
        seen_values,
      )?;
      seen_values += entry.values;
      entries.push(entry);
    }

    Ok(Self {
      entry_width,
      entries,
    })
  }
}

/// A byte range inside a chunk, relative to the chunk start.
#[derive(Debug, Clone, Copy)]
pub struct ChunkSpan {
  pub offset: u64,
  pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct Chunk {
  pub index: usize,
  pub metadata: ChunkMetadata,
  /// Absolute file offset of the chunk's first byte.
  pub start: u64,
  /// Offset relative to the start of the page's data buffer.
  pub offset: u64,
  pub num_levels: u16,
  pub rep_buffer: Option<ChunkSpan>,
  pub def_buffer: Option<ChunkSpan>,
  pub value_buffers: Vec<ChunkSpan>,
  pub header_bytes: u64,
  /// Raw repetition-index words for this chunk, when present.
  pub rep_index: Option<Vec<u64>>,
}

#[derive(Debug)]
pub struct ChunkTable {
  pub entry_width: usize,
  pub num_buffers: u32,
  pub num_items: u64,
  pub chunks: Vec<Chunk>,
  pub warnings: Vec<String>,
}

impl ChunkTable {
  pub fn total_values(&self) -> u64 {
    self.chunks.iter().map(|chunk| chunk.metadata.values).sum()
  }
}

/// Parse a mini-block page from its decoded page buffers.
///
/// This is the entry point used by `show`: it picks buffer 0 (metadata),
/// buffer 1 (data), and the optional repetition-index buffer, then fills in
/// each chunk's absolute position.
pub fn parse(
  layout: &pb21::MiniBlockLayout,
  page: &PageMeta,
  buffers: &[Vec<u8>],
  warnings: &mut Vec<String>,
) -> Result<ChunkTable> {
  let metadata = buffers
    .first()
    .ok_or_else(|| anyhow::anyhow!("mini-block page has no metadata buffer"))?;
  let data = buffers
    .get(1)
    .ok_or_else(|| anyhow::anyhow!("mini-block page has no data buffer"))?;
  let rep_index = rep_index_buffer(layout)
    .and_then(|index| buffers.get(index))
    .map(Vec::as_slice);

  let mut table = parse_buffers(metadata, data, layout, rep_index)?;
  let data_start = page
    .buffers
    .get(1)
    .map(|buffer| buffer.position)
    .unwrap_or_default();
  for chunk in &mut table.chunks {
    chunk.start = data_start + chunk.offset;
  }
  let total_values = table.total_values();
  if total_values != layout.num_items {
    warnings.push(format!(
      "chunk value counts sum to {total_values} but layout.num_items is {}",
      layout.num_items
    ));
  }
  Ok(table)
}

fn rep_index_buffer(layout: &pb21::MiniBlockLayout) -> Option<usize> {
  if layout.repetition_index_depth == 0 {
    return None;
  }
  Some(if layout.dictionary.is_some() { 3 } else { 2 })
}

fn parse_buffers(
  metadata: &[u8],
  data: &[u8],
  layout: &pb21::MiniBlockLayout,
  rep_index: Option<&[u8]>,
) -> Result<ChunkTable> {
  let ChunkMetadataTable {
    entry_width,
    entries,
  } = ChunkMetadataTable::parse(metadata, layout)?;
  let num_chunks = entries.len();

  let mut warnings = Vec::new();
  let rep_index_words = match rep_index {
    Some(bytes) => Some(parse_rep_index(
      bytes,
      num_chunks,
      layout.repetition_index_depth,
    )?),
    None => None,
  };

  let mut chunks = Vec::with_capacity(num_chunks);
  let mut offset: u64 = 0;

  for (index, chunk_metadata) in entries.into_iter().enumerate() {
    let start = offset as usize;
    let end = start
      .checked_add(chunk_metadata.bytes() as usize)
      .ok_or_else(|| anyhow::anyhow!("chunk {index} range overflows"))?;
    if end > data.len() {
      bail!(
        "chunk {index} ends at {end} but the data buffer is {} bytes",
        data.len()
      );
    }

    let mut chunk = Chunk::parse(
      index,
      &data[start..end],
      layout,
      offset,
      chunk_metadata,
      &mut warnings,
    )?;
    chunk.rep_index = rep_index_words
      .as_ref()
      .and_then(|words| words.get(index).cloned());
    chunks.push(chunk);
    offset += chunk_metadata.bytes();
  }

  if offset != data.len() as u64 {
    warnings.push(format!(
      "chunk table covers {offset} bytes but the data buffer is {} bytes",
      data.len()
    ));
  }

  Ok(ChunkTable {
    entry_width,
    num_buffers: layout.num_buffers as u32,
    num_items: layout.num_items,
    chunks,
    warnings,
  })
}

impl Chunk {
  fn parse(
    index: usize,
    chunk: &[u8],
    layout: &pb21::MiniBlockLayout,
    offset: u64,
    metadata: ChunkMetadata,
    warnings: &mut Vec<String>,
  ) -> Result<Self> {
    let nbuf = layout.num_buffers as usize;
    let size_width = if layout.has_large_chunk { 4 } else { 2 };
    let has_rep = layout.rep_compression.is_some();
    let has_def = layout.def_compression.is_some();

    let header_len = 2 + usize::from(has_rep) * 2 + usize::from(has_def) * 2 + nbuf * size_width;
    if chunk.len() < header_len {
      bail!(
        "chunk {index} is {} bytes, shorter than its {header_len}-byte header",
        chunk.len()
      );
    }

    let mut cursor = 0usize;
    let num_levels = read_u16(chunk, &mut cursor);
    let rep_bytes = if has_rep {
      read_u16(chunk, &mut cursor)
    } else {
      0
    };
    let def_bytes = if has_def {
      read_u16(chunk, &mut cursor)
    } else {
      0
    };
    let mut value_sizes = Vec::with_capacity(nbuf);
    for _ in 0..nbuf {
      value_sizes.push(if size_width == 4 {
        read_u32(chunk, &mut cursor)
      } else {
        read_u16(chunk, &mut cursor) as u32
      });
    }

    let header_bytes = align8(header_len as u64);
    let mut inner = header_bytes;

    let rep_buffer = if has_rep {
      let span = ChunkSpan {
        offset: inner,
        bytes: rep_bytes as u64,
      };
      inner = align8(inner + span.bytes);
      Some(span)
    } else {
      None
    };
    let def_buffer = if has_def {
      let span = ChunkSpan {
        offset: inner,
        bytes: def_bytes as u64,
      };
      inner = align8(inner + span.bytes);
      Some(span)
    } else {
      None
    };
    let mut value_buffers = Vec::with_capacity(nbuf);
    for size in &value_sizes {
      let span = ChunkSpan {
        offset: inner,
        bytes: *size as u64,
      };
      value_buffers.push(span);
      inner = align8(inner + span.bytes);
    }

    if inner != chunk.len() as u64 {
      warnings.push(format!(
        "chunk {index}: header/buffers describe {inner} bytes but the chunk is {} bytes",
        chunk.len()
      ));
    }
    if let Some(span) = rep_buffer
      && span.offset + span.bytes > chunk.len() as u64
    {
      warnings.push(format!(
        "chunk {index}: repetition levels are out of bounds"
      ));
    }
    if let Some(span) = def_buffer
      && span.offset + span.bytes > chunk.len() as u64
    {
      warnings.push(format!(
        "chunk {index}: definition levels are out of bounds"
      ));
    }

    Ok(Chunk {
      index,
      metadata,
      start: 0,
      offset,
      num_levels,
      rep_buffer,
      def_buffer,
      value_buffers,
      header_bytes,
      rep_index: None,
    })
  }
}

fn parse_rep_index(bytes: &[u8], num_chunks: usize, depth: u32) -> Result<Vec<Vec<u64>>> {
  let stride = depth as usize + 1;
  let expected = num_chunks * stride * 8;
  if bytes.len() != expected {
    bail!(
      "repetition index is {} bytes, expected {expected} ({num_chunks} chunks x {stride} u64)",
      bytes.len()
    );
  }
  let mut out = Vec::with_capacity(num_chunks);
  for chunk in bytes.chunks_exact(stride * 8) {
    let words = chunk
      .chunks_exact(8)
      .map(|word| u64::from_le_bytes(word.try_into().unwrap()))
      .collect();
    out.push(words);
  }
  Ok(out)
}

/// Decode a FullZip repetition index.
///
/// FullZip writes a byte-packed index with a width chosen from the maximum
/// offset: 1, 2, 4, or 8 bytes. There is one entry per row plus a final
/// sentinel, so the width can be recovered from the buffer length.
pub fn parse_full_zip_rep_index(bytes: &[u8], num_rows: u64) -> Result<(usize, Vec<u64>)> {
  let entries = num_rows as usize + 1;
  if entries == 0 || !bytes.len().is_multiple_of(entries) {
    bail!(
      "full-zip repetition index is {} bytes, not divisible into {entries} entries",
      bytes.len()
    );
  }
  let width = bytes.len() / entries;
  if !matches!(width, 1 | 2 | 4 | 8) {
    bail!("full-zip repetition index entry width must be 1/2/4/8 bytes, got {width}");
  }
  let values = bytes
    .chunks_exact(width)
    .map(|chunk| match width {
      1 => chunk[0] as u64,
      2 => u16::from_le_bytes(chunk.try_into().unwrap()) as u64,
      4 => u32::from_le_bytes(chunk.try_into().unwrap()) as u64,
      _ => u64::from_le_bytes(chunk.try_into().unwrap()),
    })
    .collect();
  Ok((width, values))
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> u16 {
  let value = u16::from_le_bytes(bytes[*cursor..*cursor + 2].try_into().unwrap());
  *cursor += 2;
  value
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> u32 {
  let value = u32::from_le_bytes(bytes[*cursor..*cursor + 4].try_into().unwrap());
  *cursor += 4;
  value
}

fn align8(value: u64) -> u64 {
  value.div_ceil(8) * 8
}

#[cfg(test)]
mod tests {
  use super::*;

  fn layout(num_items: u64) -> pb21::MiniBlockLayout {
    pb21::MiniBlockLayout {
      num_items,
      num_buffers: 1,
      ..Default::default()
    }
  }

  #[test]
  fn parses_chunk_metadata_table() {
    // Chunk 0: 16 bytes, log2=3 (8 values). Final chunk: 8 bytes, remaining 2.
    let mut metadata = Vec::new();
    for entry in [0x13u16, 0x00] {
      metadata.extend_from_slice(&entry.to_le_bytes());
    }

    let table = ChunkMetadataTable::parse(&metadata, &layout(10)).unwrap();
    assert_eq!(table.entry_width, 2);
    assert_eq!(table.entries.len(), 2);
    assert_eq!(table.entries[0].bytes(), 16);
    assert_eq!(table.entries[0].log2_values(), 3);
    assert_eq!(table.entries[0].values, 8);
    assert_eq!(table.entries[1].bytes(), 8);
    assert_eq!(table.entries[1].log2_values(), 0);
    assert_eq!(table.entries[1].values, 2);
  }

  #[test]
  fn rejects_zero_log_for_non_final_chunk() {
    let mut metadata = Vec::new();
    for entry in [0x00u16, 0x13u16] {
      metadata.extend_from_slice(&entry.to_le_bytes());
    }
    let error = ChunkMetadataTable::parse(&metadata, &layout(10)).unwrap_err();
    assert!(
      error.to_string().contains("non-final chunk 0"),
      "error: {error}"
    );
  }

  #[test]
  fn rejects_final_log_mismatch() {
    // First chunk: 8 values; final chunk claims log2=4 (16 values), only 2 remain.
    let mut metadata = Vec::new();
    for entry in [0x13u16, 0x24u16] {
      metadata.extend_from_slice(&entry.to_le_bytes());
    }
    let error = ChunkMetadataTable::parse(&metadata, &layout(10)).unwrap_err();
    assert!(error.to_string().contains("implies"), "error: {error}");
  }

  #[test]
  fn rejects_empty_metadata() {
    let error = ChunkMetadataTable::parse(&[], &layout(0)).unwrap_err();
    assert!(error.to_string().contains("no chunks"), "error: {error}");
  }
}
