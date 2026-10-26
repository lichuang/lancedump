// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! FullZip page internals: control words, repetition index, and value slicing.
//!
//! A FullZip page stores one buffer of items:
//!
//! ```text
//! [control word][length?][value] ...          (variable-width values)
//! [control word][value] ...                   (fixed-width values)
//! ```
//!
//! The control word packs `rep` and `def` into 0, 1, 2, or 4 bytes. The optional
//! second buffer is a byte-packed repetition index (one offset per row plus a
//! sentinel).

use anyhow::{Result, bail};

use crate::proto::pb21;
use crate::proto::pb21::RepDefLayer;
use crate::proto::pb21::full_zip_layout::Details;

#[derive(Debug, Clone)]
pub struct FullZipDetail {
  pub rep_levels: Vec<u16>,
  pub def_levels: Vec<u16>,
  /// One entry per item: bytes taken by the value (0 for invisible/null items).
  pub item_sizes: Vec<u64>,
  pub values: FullZipValues,
}

#[derive(Debug, Clone)]
pub enum FullZipValues {
  Fixed {
    bits_per_value: u64,
    values: Vec<u64>,
  },
  Variable {
    bits_per_offset: u64,
    values: Vec<Vec<u8>>,
  },
}

/// Decode a FullZip repetition index.
///
/// The index is byte-packed with a width chosen from the maximum offset
/// (1, 2, 4, or 8 bytes). There is one entry per row plus a final sentinel.
pub fn parse_rep_index(bytes: &[u8], num_rows: u64) -> Result<(usize, Vec<u64>)> {
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

/// Decode the item stream of a FullZip page.
pub fn parse_detail(
  layout: &pb21::FullZipLayout,
  buffers: &[Vec<u8>],
  warnings: &mut Vec<String>,
) -> Result<FullZipDetail> {
  let data = buffers
    .first()
    .ok_or_else(|| anyhow::anyhow!("full-zip page has no value buffer"))?;

  let bits_rep = layout.bits_rep as usize;
  let bits_def = layout.bits_def as usize;
  let total_bits = bits_rep + bits_def;
  let word_bytes = match total_bits {
    0 => 0,
    1..=8 => 1,
    9..=16 => 2,
    _ => 4,
  };

  let layers: Vec<RepDefLayer> = layout
    .layers
    .iter()
    .filter_map(|layer| RepDefLayer::try_from(*layer).ok())
    .collect();
  let max_visible_def: u16 = layers
    .iter()
    .take_while(|layer| !is_list(**layer))
    .map(|layer| def_levels(*layer))
    .sum();

  let mut values = match layout.details.as_ref() {
    Some(Details::BitsPerValue(bits)) => {
      if !bits.is_multiple_of(8) {
        bail!("full-zip fixed values must be byte aligned, got {bits} bits");
      }
      FullZipValues::Fixed {
        bits_per_value: *bits as u64,
        values: Vec::new(),
      }
    }
    Some(Details::BitsPerOffset(bits)) => {
      if !matches!(bits, 32 | 64) {
        bail!("full-zip variable offsets must be 32 or 64 bits, got {bits}");
      }
      if let Some(pb21::CompressiveEncoding {
        compression: Some(pb21::compressive_encoding::Compression::Variable(variable)),
      }) = layout.value_compression.as_ref()
        && variable.values.is_some()
      {
        bail!("full-zip variable values with general compression are not supported");
      }
      FullZipValues::Variable {
        bits_per_offset: *bits as u64,
        values: Vec::new(),
      }
    }
    None => bail!("full-zip layout has no value width"),
  };

  let num_items = layout.num_items as usize;
  let mut rep_levels = Vec::with_capacity(num_items);
  let mut def_levels = Vec::with_capacity(num_items);
  let mut item_sizes = Vec::with_capacity(num_items);
  let mut cursor = 0usize;

  for _ in 0..num_items {
    if cursor + word_bytes > data.len() {
      bail!("full-zip item stream ends at {cursor}, before the {num_items} declared items");
    }
    let word = read_uint(data, cursor, word_bytes);
    cursor += word_bytes;

    let rep = if bits_rep == 0 {
      0
    } else {
      ((word >> bits_def) & mask(bits_rep)) as u16
    };
    let def = if bits_def == 0 {
      0
    } else {
      (word & mask(bits_def)) as u16
    };
    let is_visible = bits_def == 0 || def <= max_visible_def;
    let is_valid = bits_def == 0 || def == 0;

    rep_levels.push(rep);
    def_levels.push(def);

    let size = match &mut values {
      FullZipValues::Fixed {
        bits_per_value,
        values,
      } => {
        if is_visible {
          let width = (*bits_per_value / 8) as usize;
          if cursor + width > data.len() {
            bail!("full-zip fixed value at offset {cursor} is out of bounds");
          }
          values.push(read_uint(data, cursor, width));
          cursor += width;
          width as u64
        } else {
          0
        }
      }
      FullZipValues::Variable {
        bits_per_offset,
        values,
      } => {
        if is_visible && is_valid {
          let width = (*bits_per_offset / 8) as usize;
          if cursor + width > data.len() {
            bail!("full-zip value length at offset {cursor} is out of bounds");
          }
          let length = read_uint(data, cursor, width) as usize;
          cursor += width;
          if cursor + length > data.len() {
            bail!("full-zip value at offset {cursor} with length {length} is out of bounds");
          }
          values.push(data[cursor..cursor + length].to_vec());
          cursor += length;
          length as u64
        } else {
          0
        }
      }
    };
    item_sizes.push(size);
  }

  if cursor != data.len() {
    warnings.push(format!(
      "full-zip item stream consumed {cursor} of {} bytes",
      data.len()
    ));
  }

  Ok(FullZipDetail {
    rep_levels,
    def_levels,
    item_sizes,
    values,
  })
}

fn is_list(layer: RepDefLayer) -> bool {
  matches!(
    layer,
    RepDefLayer::RepdefAllValidList
      | RepDefLayer::RepdefNullableList
      | RepDefLayer::RepdefEmptyableList
      | RepDefLayer::RepdefNullAndEmptyList
  )
}

fn def_levels(layer: RepDefLayer) -> u16 {
  match layer {
    RepDefLayer::RepdefAllValidItem | RepDefLayer::RepdefAllValidList => 0,
    RepDefLayer::RepdefNullableItem
    | RepDefLayer::RepdefNullableList
    | RepDefLayer::RepdefEmptyableList => 1,
    RepDefLayer::RepdefNullAndEmptyList => 2,
    RepDefLayer::RepdefUnspecified => 0,
  }
}

fn mask(bits: usize) -> u64 {
  if bits == 0 {
    0
  } else if bits >= 64 {
    u64::MAX
  } else {
    (1u64 << bits) - 1
  }
}

fn read_uint(data: &[u8], offset: usize, width: usize) -> u64 {
  let mut value = 0u64;
  for index in 0..width {
    let byte = data.get(offset + index).copied().unwrap_or(0) as u64;
    value |= byte << (index * 8);
  }
  value
}
