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

/// Decode the `Any` payload of a page encoding.
pub fn decode(encoding: &PageEncoding) -> Result<Option<DecodedLayout>> {
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
