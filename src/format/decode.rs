// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Decoding of compressed mini-block levels and values.
//!
//! This is a read-only dumper, not a full query engine: it implements the
//! compression schemes needed to inspect the common pages (flat, RLE,
//! inline/out-of-line bitpacking, fixed-size-list) and reports anything else
//! as unsupported.

use anyhow::{Result, bail};

use crate::proto::pb21;
use crate::proto::pb21::compressive_encoding::Compression as C;

/// FastLanes lane order used by Lance's bitpacking kernels.
const FL_ORDER: [usize; 8] = [0, 4, 2, 6, 1, 5, 3, 7];

#[derive(Debug, Clone)]
pub struct DecodedValues {
  pub bits_per_value: u64,
  pub items_per_value: u64,
  pub values: Vec<u64>,
}

/// Decode rep/def levels. Levels are always 16-bit; the returned vector has
/// exactly `num_values` entries.
pub fn decode_levels(
  compression: &pb21::CompressiveEncoding,
  data: &[u8],
  num_values: u64,
) -> Result<Vec<u16>> {
  let values = match compression.compression.as_ref() {
    Some(C::Flat(flat)) => decode_flat(data, flat.bits_per_value, num_values)?,
    Some(C::Rle(rle)) => {
      let value_bits = flat_bits(rle.values.as_deref())?;
      let run_bits = flat_bits(rle.run_lengths.as_deref())?;
      decode_rle(data, num_values, value_bits, run_bits)?
    }
    Some(C::OutOfLineBitpacking(bitpacking)) => {
      let compressed_bits = flat_bits(bitpacking.values.as_deref())?;
      unpack_out_of_line(
        data,
        bitpacking.uncompressed_bits_per_value,
        compressed_bits,
        num_values,
      )?
    }
    Some(C::InlineBitpacking(bitpacking)) => {
      unpack_inline(data, bitpacking.uncompressed_bits_per_value, num_values)?
    }
    Some(_) => bail!(
      "unsupported level compression: {}",
      compression_name(Some(compression))
    ),
    None => bail!("missing level compression"),
  };
  Ok(values.into_iter().map(|value| value as u16).collect())
}

/// Decode the values of one mini-block chunk.
///
/// `buffers` are the chunk's value buffers. The supported encodings use a
/// single buffer; multi-buffer encodings (packed structs) are reported as
/// unsupported.
pub fn decode_values(
  compression: &pb21::CompressiveEncoding,
  buffers: &[&[u8]],
  num_values: u64,
) -> Result<DecodedValues> {
  let data = buffers
    .first()
    .ok_or_else(|| anyhow::anyhow!("value compression has no buffers"))?;
  match compression.compression.as_ref() {
    Some(C::Flat(flat)) => Ok(DecodedValues {
      bits_per_value: flat.bits_per_value,
      items_per_value: 1,
      values: decode_flat(data, flat.bits_per_value, num_values)?,
    }),
    Some(C::InlineBitpacking(bitpacking)) => Ok(DecodedValues {
      bits_per_value: bitpacking.uncompressed_bits_per_value,
      items_per_value: 1,
      values: unpack_inline(data, bitpacking.uncompressed_bits_per_value, num_values)?,
    }),
    Some(C::OutOfLineBitpacking(bitpacking)) => {
      let compressed_bits = flat_bits(bitpacking.values.as_deref())?;
      Ok(DecodedValues {
        bits_per_value: bitpacking.uncompressed_bits_per_value,
        items_per_value: 1,
        values: unpack_out_of_line(
          data,
          bitpacking.uncompressed_bits_per_value,
          compressed_bits,
          num_values,
        )?,
      })
    }
    Some(C::FixedSizeList(fsl)) => {
      let child = fsl
        .values
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("fixed-size-list compression has no child"))?;
      let items = fsl.items_per_value;
      let mut decoded = decode_values(child, buffers, num_values.saturating_mul(items))?;
      decoded.items_per_value = decoded.items_per_value.saturating_mul(items);
      if fsl.has_validity {
        bail!("fixed-size-list validity is not supported by --detail");
      }
      Ok(decoded)
    }
    Some(_) => bail!(
      "unsupported value compression: {}",
      compression_name(Some(compression))
    ),
    None => bail!("missing value compression"),
  }
}

fn flat_bits(encoding: Option<&pb21::CompressiveEncoding>) -> Result<u64> {
  match encoding.and_then(|encoding| encoding.compression.as_ref()) {
    Some(C::Flat(flat)) => Ok(flat.bits_per_value),
    Some(_) => bail!("expected a flat child encoding"),
    None => bail!("missing child encoding"),
  }
}

fn compression_name(encoding: Option<&pb21::CompressiveEncoding>) -> String {
  use pb21::compressive_encoding::Compression as C;
  let Some(encoding) = encoding else {
    return "none".to_string();
  };
  match encoding.compression.as_ref() {
    Some(C::Flat(_)) => "flat",
    Some(C::Variable(_)) => "variable",
    Some(C::Constant(_)) => "constant",
    Some(C::OutOfLineBitpacking(_)) => "bitpacking(out-of-line)",
    Some(C::InlineBitpacking(_)) => "bitpacking(inline)",
    Some(C::Fsst(_)) => "fsst",
    Some(C::Dictionary(_)) => "dictionary",
    Some(C::Rle(_)) => "rle",
    Some(C::ByteStreamSplit(_)) => "byte-stream-split",
    Some(C::General(_)) => "general",
    Some(C::FixedSizeList(_)) => "fixed-size-list",
    Some(C::PackedStruct(_)) => "packed-struct",
    Some(C::VariablePackedStruct(_)) => "variable-packed-struct",
    None => "unspecified",
  }
  .to_string()
}

fn decode_flat(data: &[u8], bits_per_value: u64, num_values: u64) -> Result<Vec<u64>> {
  let num_values = num_values as usize;
  if bits_per_value == 0 {
    return Ok(vec![0; num_values]);
  }
  if bits_per_value.is_multiple_of(8) {
    let width = (bits_per_value / 8) as usize;
    if data.len() < num_values.saturating_mul(width) {
      bail!(
        "flat buffer is {} bytes, expected at least {}",
        data.len(),
        num_values * width
      );
    }
    return Ok(
      (0..num_values)
        .map(|index| read_uint(data, index * width, width))
        .collect(),
    );
  }

  let width = bits_per_value as usize;
  if data.len() * 8 < num_values.saturating_mul(width) {
    bail!(
      "flat bitstream is {} bits, expected at least {}",
      data.len() * 8,
      num_values * width
    );
  }
  let mut values = Vec::with_capacity(num_values);
  let mut bit = 0usize;
  for _ in 0..num_values {
    values.push(read_bits(data, bit, width));
    bit += width;
  }
  Ok(values)
}

/// RLE block frame: `[values_size: u64-le][values][run lengths]`.
fn decode_rle(
  data: &[u8],
  num_values: u64,
  value_bits: u64,
  run_length_bits: u64,
) -> Result<Vec<u64>> {
  if data.len() < 8 {
    bail!(
      "RLE buffer is {} bytes, shorter than its 8-byte header",
      data.len()
    );
  }
  if !value_bits.is_multiple_of(8) || !run_length_bits.is_multiple_of(8) {
    bail!(
      "RLE child widths must be byte-aligned, got values={value_bits}, run_lengths={run_length_bits}"
    );
  }
  let values_size = u64::from_le_bytes(data[..8].try_into().unwrap()) as usize;
  if 8 + values_size > data.len() {
    bail!("RLE values buffer size {values_size} exceeds the RLE buffer");
  }
  let values = &data[8..8 + values_size];
  let lengths = &data[8 + values_size..];
  let value_width = (value_bits / 8) as usize;
  let length_width = (run_length_bits / 8) as usize;

  let num_runs = values.len() / value_width;
  if !values.len().is_multiple_of(value_width)
    || !lengths.len().is_multiple_of(length_width)
    || lengths.len() / length_width != num_runs
  {
    bail!(
      "inconsistent RLE buffers: {} value bytes, {} length bytes",
      values.len(),
      lengths.len()
    );
  }

  let mut out = Vec::with_capacity(num_values as usize);
  for run in 0..num_runs {
    let value = read_uint(values, run * value_width, value_width);
    let count = read_uint(lengths, run * length_width, length_width);
    for _ in 0..count {
      out.push(value);
    }
  }
  if out.len() != num_values as usize {
    bail!("RLE expands to {} values, expected {num_values}", out.len());
  }
  Ok(out)
}

/// Inline bitpacking: `[width word][1024 packed values]` per mini-block chunk.
fn unpack_inline(data: &[u8], uncompressed_bits: u64, num_values: u64) -> Result<Vec<u64>> {
  if num_values == 0 {
    return Ok(Vec::new());
  }
  if !uncompressed_bits.is_multiple_of(8) {
    bail!("inline bitpacking word width must be a multiple of 8");
  }
  let word_bytes = (uncompressed_bits / 8) as usize;
  if data.len() < word_bytes {
    bail!("inline bitpacking chunk is shorter than its {word_bytes}-byte header");
  }
  let width = read_uint(data, 0, word_bytes) as usize;
  if width > uncompressed_bits as usize {
    bail!("inline bitpacking width {width} exceeds {uncompressed_bits} bits");
  }
  let values = unpack_1024(&data[word_bytes..], width, uncompressed_bits as usize)?;
  if num_values as usize > values.len() {
    bail!(
      "inline bitpacking chunk holds {} values, requested {num_values}",
      values.len()
    );
  }
  Ok(values[..num_values as usize].to_vec())
}

/// Out-of-line bitpacking: fixed-size 1024-value chunks, optionally a raw tail.
fn unpack_out_of_line(
  data: &[u8],
  uncompressed_bits: u64,
  compressed_bits: u64,
  num_values: u64,
) -> Result<Vec<u64>> {
  if !uncompressed_bits.is_multiple_of(8) {
    bail!("bitpacking word width must be a multiple of 8");
  }
  let word_bytes = (uncompressed_bits / 8) as usize;
  let words_per_chunk = (1024 * compressed_bits as usize).div_ceil(uncompressed_bits as usize);
  let num_whole = (num_values / 1024) as usize;
  let tail = (num_values % 1024) as usize;
  let total_words = data.len() / word_bytes;
  let expected_full_words = num_whole * words_per_chunk;
  let tail_is_raw = tail > 0 && total_words == expected_full_words + tail;

  let mut out = Vec::with_capacity(num_values as usize);
  for chunk in 0..num_whole {
    let start = chunk * words_per_chunk * word_bytes;
    let end = start + words_per_chunk * word_bytes;
    if end > data.len() {
      bail!("out-of-line bitpacking chunk {chunk} is out of bounds");
    }
    out.extend(unpack_1024(
      &data[start..end],
      compressed_bits as usize,
      uncompressed_bits as usize,
    )?);
  }

  if tail > 0 {
    if tail_is_raw {
      for index in 0..tail {
        let offset = (expected_full_words + index) * word_bytes;
        out.push(read_uint(data, offset, word_bytes));
      }
    } else {
      let start = expected_full_words * word_bytes;
      if start + words_per_chunk * word_bytes > data.len() {
        bail!("out-of-line bitpacking tail is out of bounds");
      }
      let mut values = unpack_1024(
        &data[start..start + words_per_chunk * word_bytes],
        compressed_bits as usize,
        uncompressed_bits as usize,
      )?;
      values.truncate(tail);
      out.extend(values);
    }
  }

  if out.len() != num_values as usize {
    bail!(
      "bitpacking decoded {} values, expected {num_values}",
      out.len()
    );
  }
  Ok(out)
}

/// Unpack one FastLanes bitpacked block of 1024 values.
fn unpack_1024(data: &[u8], width: usize, word_bits: usize) -> Result<Vec<u64>> {
  let word_bytes = word_bits / 8;
  let lanes = 1024 / word_bits;
  let expected = lanes * width * word_bytes;
  if data.len() < expected {
    bail!(
      "bitpacked chunk is {} bytes, expected at least {expected} for width {width}",
      data.len()
    );
  }

  let mut out = vec![0u64; 1024];
  if width == 0 {
    return Ok(out);
  }

  let read_word = |word: usize| read_uint(data, word * word_bytes, word_bytes);

  if width == word_bits {
    for row in 0..word_bits {
      for lane in 0..lanes {
        out[fl_index(row, lane)] = read_word(lanes * row + lane);
      }
    }
    return Ok(out);
  }

  let mask = |bits: usize| -> u64 {
    if bits == 0 {
      0
    } else if bits >= 64 {
      u64::MAX
    } else {
      (1u64 << bits) - 1
    }
  };

  for lane in 0..lanes {
    let mut src = read_word(lane);
    for row in 0..word_bits {
      let curr_word = (row * width) / word_bits;
      let next_word = ((row + 1) * width) / word_bits;
      let shift = (row * width) % word_bits;
      let value = if next_word > curr_word {
        let remaining_bits = ((row + 1) * width) % word_bits;
        let current_bits = width - remaining_bits;
        let mut value = (src >> shift) & mask(current_bits);
        if next_word < width {
          src = read_word(lanes * next_word + lane);
          value |= (src & mask(remaining_bits)) << current_bits;
        }
        value
      } else {
        (src >> shift) & mask(width)
      };
      out[fl_index(row, lane)] = value;
    }
  }
  Ok(out)
}

fn fl_index(row: usize, lane: usize) -> usize {
  FL_ORDER[row / 8] * 16 + (row % 8) * 128 + lane
}

fn read_uint(data: &[u8], offset: usize, width: usize) -> u64 {
  let mut value = 0u64;
  for index in 0..width {
    let byte = data.get(offset + index).copied().unwrap_or(0) as u64;
    value |= byte << (index * 8);
  }
  value
}

fn read_bits(data: &[u8], bit: usize, width: usize) -> u64 {
  let byte = bit / 8;
  let shift = bit % 8;
  let mut word = 0u128;
  for index in 0..9 {
    if let Some(value) = data.get(byte + index) {
      word |= (*value as u128) << (index * 8);
    }
  }
  let mask = if width >= 64 {
    u64::MAX as u128
  } else {
    (1u128 << width) - 1
  };
  ((word >> shift) & mask) as u64
}
