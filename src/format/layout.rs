// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Page layout decoding (`Any` -> `PageLayout` / legacy `ArrayEncoding`).

use anyhow::{Context, Result};
use prost::Message;

use crate::format::container::PageEncoding;
use crate::proto::{pb20, pb21};

#[derive(Debug)]
pub enum DecodedLayout {
  /// Structural layout used by v2.1+.
  Structural(pb21::PageLayout),
  /// Legacy array encoding used by v2.0.
  Legacy(pb20::ArrayEncoding),
}

#[derive(Debug, Clone)]
pub struct LayoutSummary {
  pub kind: &'static str,
  pub detail: String,
}

#[derive(Debug, Clone)]
pub struct LayoutDetail {
  pub kind: &'static str,
  pub fields: Vec<(String, String)>,
}

/// Decode the `Any` payload of a page encoding.
pub fn decode_layout(encoding: &PageEncoding) -> Result<Option<DecodedLayout>> {
  let PageEncoding::Direct { type_url, value } = encoding else {
    return Ok(None);
  };
  match crate::proto::any_type_name(type_url) {
    "PageLayout" => {
      let layout = pb21::PageLayout::decode(value.as_slice())
        .with_context(|| format!("failed to decode PageLayout from {type_url}"))?;
      Ok(Some(DecodedLayout::Structural(layout)))
    }
    "ArrayEncoding" => {
      let layout = pb20::ArrayEncoding::decode(value.as_slice())
        .with_context(|| format!("failed to decode ArrayEncoding from {type_url}"))?;
      Ok(Some(DecodedLayout::Legacy(layout)))
    }
    _ => Ok(None),
  }
}

pub fn summary(decoded: &DecodedLayout) -> LayoutSummary {
  match decoded {
    DecodedLayout::Structural(layout) => match layout.layout.as_ref() {
      Some(pb21::page_layout::Layout::MiniBlockLayout(mini)) => LayoutSummary {
        kind: "mini-block",
        detail: format!(
          "{}, {} value buffer(s)",
          if mini.has_large_chunk {
            "u32 chunk meta"
          } else {
            "u16 chunk meta"
          },
          mini.num_buffers
        ),
      },
      Some(pb21::page_layout::Layout::FullZipLayout(full_zip)) => {
        let width = match full_zip.details.as_ref() {
          Some(pb21::full_zip_layout::Details::BitsPerValue(bits)) => {
            format!("fixed {bits} bits/value")
          }
          Some(pb21::full_zip_layout::Details::BitsPerOffset(bits)) => {
            format!("variable {bits} bits/offset")
          }
          None => "unknown width".to_string(),
        };
        LayoutSummary {
          kind: "full-zip",
          detail: width,
        }
      }
      Some(pb21::page_layout::Layout::SparseLayout(sparse)) => LayoutSummary {
        kind: "sparse",
        detail: format!(
          "{} layer(s), {} value buffer(s)",
          sparse.structural_layers.len(),
          sparse.num_buffers
        ),
      },
      Some(pb21::page_layout::Layout::ConstantLayout(constant)) => LayoutSummary {
        kind: "constant",
        detail: if constant.inline_value.is_some() {
          "inline scalar".to_string()
        } else {
          "all-null".to_string()
        },
      },
      Some(pb21::page_layout::Layout::BlobLayout(blob)) => LayoutSummary {
        kind: "blob",
        detail: blob
          .inner_layout
          .as_ref()
          .map(|inner| format!("inner={}", inner_kind(inner)))
          .unwrap_or_else(|| "inner=<none>".to_string()),
      },
      None => LayoutSummary {
        kind: "structural",
        detail: "<no layout>".to_string(),
      },
    },
    DecodedLayout::Legacy(encoding) => LayoutSummary {
      kind: "legacy-v2.0",
      detail: legacy_kind(encoding),
    },
  }
}

/// A flat key/value description used by `ldp show`.
pub fn describe(decoded: &DecodedLayout) -> LayoutDetail {
  match decoded {
    DecodedLayout::Structural(layout) => match layout.layout.as_ref() {
      Some(pb21::page_layout::Layout::MiniBlockLayout(mini)) => LayoutDetail {
        kind: "mini-block",
        fields: vec![
          (
            "chunk metadata width".to_string(),
            if mini.has_large_chunk {
              "u32 (4 bytes)"
            } else {
              "u16 (2 bytes)"
            }
            .to_string(),
          ),
          ("num_buffers".to_string(), mini.num_buffers.to_string()),
          ("num_items".to_string(), mini.num_items.to_string()),
          (
            "rep_compression".to_string(),
            compression_name(mini.rep_compression.as_ref()),
          ),
          (
            "def_compression".to_string(),
            compression_name(mini.def_compression.as_ref()),
          ),
          (
            "value_compression".to_string(),
            compression_name(mini.value_compression.as_ref()),
          ),
          (
            "dictionary".to_string(),
            match mini.dictionary.as_ref() {
              Some(dictionary) => format!(
                "{} ({} items)",
                compression_name(Some(dictionary)),
                mini.num_dictionary_items
              ),
              None => "none".to_string(),
            },
          ),
          (
            "repetition_index_depth".to_string(),
            mini.repetition_index_depth.to_string(),
          ),
          ("layers".to_string(), layers_name(&mini.layers)),
        ],
      },
      Some(pb21::page_layout::Layout::FullZipLayout(full_zip)) => {
        let mut fields = vec![
          ("bits_rep".to_string(), full_zip.bits_rep.to_string()),
          ("bits_def".to_string(), full_zip.bits_def.to_string()),
          ("num_items".to_string(), full_zip.num_items.to_string()),
          (
            "num_visible_items".to_string(),
            full_zip.num_visible_items.to_string(),
          ),
          (
            "value_compression".to_string(),
            compression_name(full_zip.value_compression.as_ref()),
          ),
          ("layers".to_string(), layers_name(&full_zip.layers)),
        ];
        match full_zip.details.as_ref() {
          Some(pb21::full_zip_layout::Details::BitsPerValue(bits)) => fields.insert(
            0,
            ("value width".to_string(), format!("{bits} bits (fixed)")),
          ),
          Some(pb21::full_zip_layout::Details::BitsPerOffset(bits)) => fields.insert(
            0,
            (
              "offset width".to_string(),
              format!("{bits} bits (variable)"),
            ),
          ),
          None => {}
        }
        LayoutDetail {
          kind: "full-zip",
          fields,
        }
      }
      Some(pb21::page_layout::Layout::SparseLayout(sparse)) => LayoutDetail {
        kind: "sparse",
        fields: vec![
          ("num_buffers".to_string(), sparse.num_buffers.to_string()),
          ("num_items".to_string(), sparse.num_items.to_string()),
          (
            "num_visible_items".to_string(),
            sparse.num_visible_items.to_string(),
          ),
          (
            "chunk metadata width".to_string(),
            if sparse.has_large_chunk { "u32" } else { "u16" }.to_string(),
          ),
          (
            "value_compression".to_string(),
            compression_name(sparse.value_compression.as_ref()),
          ),
          (
            "structural_layers".to_string(),
            sparse.structural_layers.len().to_string(),
          ),
        ],
      },
      Some(pb21::page_layout::Layout::ConstantLayout(constant)) => LayoutDetail {
        kind: "constant",
        fields: vec![
          (
            "inline_value".to_string(),
            match constant.inline_value.as_ref() {
              Some(value) => format!("{} byte(s): {}", value.len(), hex_preview(value)),
              None => "none (all-null page)".to_string(),
            },
          ),
          (
            "rep_compression".to_string(),
            compression_name(constant.rep_compression.as_ref()),
          ),
          (
            "def_compression".to_string(),
            compression_name(constant.def_compression.as_ref()),
          ),
          (
            "num_rep_values".to_string(),
            constant.num_rep_values.to_string(),
          ),
          (
            "num_def_values".to_string(),
            constant.num_def_values.to_string(),
          ),
          ("layers".to_string(), layers_name(&constant.layers)),
        ],
      },
      Some(pb21::page_layout::Layout::BlobLayout(blob)) => {
        let mut fields = vec![("layers".to_string(), layers_name(&blob.layers))];
        if let Some(inner) = blob.inner_layout.as_ref() {
          fields.push(("inner layout".to_string(), inner_kind(inner).to_string()));
        }
        LayoutDetail {
          kind: "blob",
          fields,
        }
      }
      None => LayoutDetail {
        kind: "structural",
        fields: vec![("layout".to_string(), "<none>".to_string())],
      },
    },
    DecodedLayout::Legacy(encoding) => LayoutDetail {
      kind: "legacy-v2.0",
      fields: vec![
        ("array encoding".to_string(), legacy_kind(encoding)),
        (
          "note".to_string(),
          "legacy v2.0 encoding tree; ldp only summarizes it".to_string(),
        ),
      ],
    },
  }
}

/// Render the full protobuf debug form (used by `--raw`).
pub fn raw(decoded: &DecodedLayout) -> String {
  match decoded {
    DecodedLayout::Structural(layout) => format!("{layout:#?}"),
    DecodedLayout::Legacy(encoding) => format!("{encoding:#?}"),
  }
}

fn inner_kind(layout: &pb21::PageLayout) -> &'static str {
  match layout.layout.as_ref() {
    Some(pb21::page_layout::Layout::MiniBlockLayout(_)) => "mini-block",
    Some(pb21::page_layout::Layout::FullZipLayout(_)) => "full-zip",
    Some(pb21::page_layout::Layout::SparseLayout(_)) => "sparse",
    Some(pb21::page_layout::Layout::ConstantLayout(_)) => "constant",
    Some(pb21::page_layout::Layout::BlobLayout(_)) => "blob",
    None => "<none>",
  }
}

fn legacy_kind(encoding: &pb20::ArrayEncoding) -> String {
  use pb20::array_encoding::ArrayEncoding as E;
  match encoding.array_encoding.as_ref() {
    Some(E::Flat(_)) => "flat",
    Some(E::Nullable(_)) => "nullable",
    Some(E::List(_)) => "list",
    Some(E::FixedSizeList(_)) => "fixed-size-list",
    Some(E::Struct(_)) => "struct",
    Some(E::Binary(_)) => "binary",
    Some(E::Dictionary(_)) => "dictionary",
    Some(E::Fsst(_)) => "fsst",
    Some(E::PackedStruct(_)) => "packed-struct",
    Some(E::Bitpacked(_)) => "bitpacked",
    Some(E::FixedSizeBinary(_)) => "fixed-size-binary",
    Some(E::BitpackedForNonNeg(_)) => "bitpacked-non-neg",
    Some(E::Constant(_)) => "constant",
    Some(_) => "other",
    None => "<none>",
  }
  .to_string()
}

pub fn compression_name(encoding: Option<&pb21::CompressiveEncoding>) -> String {
  use pb21::compressive_encoding::Compression as C;
  let Some(encoding) = encoding else {
    return "none".to_string();
  };
  match encoding.compression.as_ref() {
    Some(C::Flat(flat)) => format!("flat({} bits)", flat.bits_per_value),
    Some(C::Variable(_)) => "variable".to_string(),
    Some(C::Constant(_)) => "constant".to_string(),
    Some(C::OutOfLineBitpacking(_)) => "bitpacking(out-of-line)".to_string(),
    Some(C::InlineBitpacking(_)) => "bitpacking(inline)".to_string(),
    Some(C::Fsst(_)) => "fsst".to_string(),
    Some(C::Dictionary(_)) => "dictionary".to_string(),
    Some(C::Rle(_)) => "rle".to_string(),
    Some(C::ByteStreamSplit(_)) => "byte-stream-split".to_string(),
    Some(C::General(general)) => {
      let scheme = general
        .compression
        .as_ref()
        .map(|compression| match compression.scheme {
          1 => "lz4",
          2 => "zstd",
          _ => "unspecified",
        })
        .unwrap_or("unspecified");
      format!("general({scheme})")
    }
    Some(C::FixedSizeList(_)) => "fixed-size-list".to_string(),
    Some(C::PackedStruct(_)) => "packed-struct".to_string(),
    Some(C::VariablePackedStruct(_)) => "variable-packed-struct".to_string(),
    None => "unspecified".to_string(),
  }
}

fn layers_name(layers: &[i32]) -> String {
  if layers.is_empty() {
    return "[]".to_string();
  }
  let joined = layers
    .iter()
    .map(|layer| {
      let name = match pb21::RepDefLayer::try_from(*layer) {
        Ok(pb21::RepDefLayer::RepdefAllValidItem) => "AllValidItem",
        Ok(pb21::RepDefLayer::RepdefAllValidList) => "AllValidList",
        Ok(pb21::RepDefLayer::RepdefNullableItem) => "NullableItem",
        Ok(pb21::RepDefLayer::RepdefNullableList) => "NullableList",
        Ok(pb21::RepDefLayer::RepdefEmptyableList) => "EmptyableList",
        Ok(pb21::RepDefLayer::RepdefNullAndEmptyList) => "NullAndEmptyList",
        Ok(pb21::RepDefLayer::RepdefUnspecified) | Err(_) => "Unspecified",
      };
      format!("'{name}'")
    })
    .collect::<Vec<_>>()
    .join(", ");
  format!("[{joined}]")
}

pub fn hex_preview(bytes: &[u8]) -> String {
  const MAX: usize = 32;
  let shown = &bytes[..bytes.len().min(MAX)];
  let mut out = shown
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect::<Vec<_>>()
    .join(" ");
  if bytes.len() > MAX {
    out.push_str(" ...");
  }
  out
}
