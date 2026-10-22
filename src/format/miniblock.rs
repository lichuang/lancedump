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
//!             bits [0..4]: log2(number of values), 0 for the final chunk
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

use crate::proto::pb21;

#[derive(Debug, Clone)]
pub struct Chunk {
  pub index: usize,
  /// Offset relative to the start of the page's data buffer.
  pub offset: u64,
  pub bytes: u64,
  pub values: u64,
  pub log2_values: u8,
  pub num_levels: u16,
  pub rep_bytes: u16,
  pub def_bytes: u16,
  pub value_sizes: Vec<u32>,
  pub header_bytes: u64,
  pub rep_range: Option<(u64, u64)>,
  pub def_range: Option<(u64, u64)>,
  pub value_ranges: Vec<(u64, u64)>,
  /// Raw repetition-index words for this chunk, when present.
  pub rep_index: Option<Vec<u64>>,
}

#[derive(Debug)]
pub struct ChunkTable {
  pub entry_width: usize,
  pub num_buffers: u32,
  pub chunks: Vec<Chunk>,
  pub warnings: Vec<String>,
}

impl ChunkTable {
  pub fn total_values(&self) -> u64 {
    self.chunks.iter().map(|chunk| chunk.values).sum()
  }
}

/// Parse the mini-block chunk table.
///
/// `metadata` is page buffer 0, `data` is page buffer 1, and `rep_index` is the
/// page's repetition-index buffer when `layout.repetition_index_depth > 0`.
pub fn parse(
  metadata: &[u8],
  data: &[u8],
  layout: &pb21::MiniBlockLayout,
  rep_index: Option<&[u8]>,
) -> Result<ChunkTable> {
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

  let mut warnings = Vec::new();
  let entries: Vec<u64> = metadata
    .chunks_exact(entry_width)
    .map(|chunk| {
      if entry_width == 4 {
        u32::from_le_bytes(chunk.try_into().unwrap()) as u64
      } else {
        u16::from_le_bytes(chunk.try_into().unwrap()) as u64
      }
    })
    .collect();

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
  let mut seen_values: u64 = 0;

  for (index, entry) in entries.iter().copied().enumerate() {
    let bytes = ((entry >> 4) + 1) * 8;
    let log2_values = (entry & 0xF) as u8;

    let start = offset as usize;
    let end = start
      .checked_add(bytes as usize)
      .ok_or_else(|| anyhow::anyhow!("chunk {index} range overflows"))?;
    if end > data.len() {
      bail!(
        "chunk {index} ends at {end} but the data buffer is {} bytes",
        data.len()
      );
    }
    let chunk = &data[start..end];

    let values = if index + 1 == num_chunks {
      layout.num_items.saturating_sub(seen_values)
    } else {
      if log2_values >= 63 {
        bail!("chunk {index} has an invalid log2 value count: {log2_values}");
      }
      1u64 << log2_values
    };
    seen_values = seen_values.saturating_add(values);

    let mut parsed = parse_chunk_header(
      index,
      chunk,
      layout,
      offset,
      values,
      log2_values,
      &mut warnings,
    )?;
    parsed.rep_index = rep_index_words
      .as_ref()
      .and_then(|words| words.get(index).cloned());
    chunks.push(parsed);
    offset += bytes;
  }

  if offset != data.len() as u64 {
    warnings.push(format!(
      "chunk table covers {offset} bytes but the data buffer is {} bytes",
      data.len()
    ));
  }
  if seen_values != layout.num_items {
    warnings.push(format!(
      "chunk value counts sum to {seen_values} but layout.num_items is {}",
      layout.num_items
    ));
  }

  Ok(ChunkTable {
    entry_width,
    num_buffers: layout.num_buffers as u32,
    chunks,
    warnings,
  })
}

#[allow(clippy::too_many_arguments)]
fn parse_chunk_header(
  index: usize,
  chunk: &[u8],
  layout: &pb21::MiniBlockLayout,
  offset: u64,
  values: u64,
  log2_values: u8,
  warnings: &mut Vec<String>,
) -> Result<Chunk> {
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

  let rep_range = if has_rep {
    let range = (inner, rep_bytes as u64);
    inner = align8(inner + rep_bytes as u64);
    Some(range)
  } else {
    None
  };
  let def_range = if has_def {
    let range = (inner, def_bytes as u64);
    inner = align8(inner + def_bytes as u64);
    Some(range)
  } else {
    None
  };
  let mut value_ranges = Vec::with_capacity(nbuf);
  for size in &value_sizes {
    value_ranges.push((inner, *size as u64));
    inner = align8(inner + *size as u64);
  }

  if inner != chunk.len() as u64 {
    warnings.push(format!(
      "chunk {index}: header/buffers describe {inner} bytes but the chunk is {} bytes",
      chunk.len()
    ));
  }
  if let Some((start, len)) = rep_range
    && start + len > chunk.len() as u64
  {
    warnings.push(format!(
      "chunk {index}: repetition levels are out of bounds"
    ));
  }
  if let Some((start, len)) = def_range
    && start + len > chunk.len() as u64
  {
    warnings.push(format!(
      "chunk {index}: definition levels are out of bounds"
    ));
  }

  Ok(Chunk {
    index,
    offset,
    bytes: chunk.len() as u64,
    values,
    log2_values,
    num_levels,
    rep_bytes,
    def_bytes,
    value_sizes,
    header_bytes,
    rep_range,
    def_range,
    value_ranges,
    rep_index: None,
  })
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
