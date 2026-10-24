// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::fmt::Write;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::format::container::{FileMeta, PageMeta};
use crate::format::layout::{self, DecodedLayout};
use crate::format::miniblock;
use crate::proto::pb21;
use crate::report::Report;
use crate::source::FileSource;
use crate::view::{Style, Table, format_bytes, render_warnings};

#[derive(Debug, Clone, Copy)]
pub struct ShowOptions {
  pub hex: bool,
  pub hex_limit: usize,
  pub raw: bool,
}

#[derive(Debug, Serialize)]
pub struct ReportField {
  pub name: String,
  pub value: String,
}

#[derive(Debug, Serialize)]
pub struct LayoutReport {
  pub kind: String,
  pub fields: Vec<ReportField>,
}

#[derive(Debug, Serialize)]
pub struct PageBufferReport {
  pub index: usize,
  pub role: String,
  pub start: u64,
  pub end: u64,
  pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct ChunkReport {
  pub index: usize,
  /// Offset relative to the start of the page's data buffer.
  pub offset: u64,
  /// Absolute file offset (`data buffer start + offset`).
  pub start: u64,
  /// Raw 2-byte (v2.1) or 4-byte (v2.2+) metadata word.
  pub metadata_word: u64,
  pub bytes: u64,
  pub values: u64,
  pub log2_values: u8,
  pub num_levels: u16,
  pub rep_buffer: Option<[u64; 2]>,
  pub def_buffer: Option<[u64; 2]>,
  pub header_bytes: u64,
  pub value_buffers: Vec<[u64; 2]>,
  /// `depth + 1` words: rows ending, leftover items, ... (see the format spec).
  pub rep_index: Option<Vec<u64>>,
}

#[derive(Debug, Serialize)]
pub struct ChunkTableReport {
  pub entry_width: usize,
  pub num_buffers: u32,
  pub num_chunks: usize,
  pub num_items: u64,
  pub total_values: u64,
  pub chunks: Vec<ChunkReport>,
  pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RepIndexReport {
  pub entry_width: usize,
  pub num_entries: usize,
  pub values: Vec<u64>,
}

/// Parsed page-internal structure. Only layouts with decoded internals get a
/// variant; everything else (constant, legacy, sparse, parse failures) is
/// `None` and reported through warnings.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PageInternals {
  MiniBlock { chunks: ChunkTableReport },
  FullZip { rep_index: Option<RepIndexReport> },
}

#[derive(Debug, Serialize)]
pub struct ShowReport {
  /// Global page index (physical order), the value `--page` selects.
  pub index: u32,
  pub column: u32,
  /// Page index within the column.
  pub page: u32,
  pub start: u64,
  pub end: u64,
  pub bytes: u64,
  pub rows: u64,
  pub priority: u64,
  pub encoding: String,
  pub type_url: Option<String>,
  pub layout: Option<LayoutReport>,
  pub buffers: Vec<PageBufferReport>,
  pub internals: Option<PageInternals>,
  pub hexdump: Vec<String>,
  pub raw: Option<String>,
  pub warnings: Vec<String>,
}

impl ShowReport {
  pub fn from_meta(
    meta: &FileMeta,
    page_index: u32,
    source: &mut FileSource,
    options: ShowOptions,
  ) -> Result<Self> {
    let entries = meta.pages_in_physical_order();
    let Some((column_meta, page)) = entries.get(page_index as usize).copied() else {
      bail!(
        "page {page_index} does not exist (this file has {} page(s): 0..{})",
        entries.len(),
        entries.len().saturating_sub(1)
      );
    };

    let mut warnings = meta.warnings.clone();
    let buffers = read_buffers(source, page)?;

    let decoded = match layout::decode_layout(&page.encoding) {
      Ok(decoded) => decoded,
      Err(error) => {
        warnings.push(format!("{error:#}"));
        None
      }
    };

    let layout = decoded.as_ref().map(|decoded| {
      let detail = layout::describe(decoded);
      LayoutReport {
        kind: detail.kind.to_string(),
        fields: detail
          .fields
          .into_iter()
          .map(|(name, value)| ReportField { name, value })
          .collect(),
      }
    });

    let internals = match decoded.as_ref() {
      Some(DecodedLayout::Structural(layout)) => match layout.layout.as_ref() {
        Some(pb21::page_layout::Layout::MiniBlockLayout(mini)) => {
          match miniblock::parse(mini, page, &buffers, &mut warnings) {
            Ok(table) => Some(PageInternals::MiniBlock {
              chunks: chunk_table_report(table),
            }),
            Err(error) => {
              warnings.push(format!("failed to decode mini-block chunk table: {error}"));
              None
            }
          }
        }
        Some(pb21::page_layout::Layout::FullZipLayout(full_zip)) => Some(PageInternals::FullZip {
          rep_index: try_full_zip_rep_index(full_zip, &buffers, page.num_rows, &mut warnings),
        }),
        _ => None,
      },
      _ => None,
    };

    let roles = buffer_roles(decoded.as_ref(), page.buffers.len());
    let page_buffers = page
      .buffers
      .iter()
      .enumerate()
      .map(|(index, buffer)| PageBufferReport {
        index,
        role: roles
          .get(index)
          .cloned()
          .unwrap_or_else(|| format!("buffer {index}")),
        start: buffer.position,
        end: buffer.end(),
        size: buffer.size,
      })
      .collect();

    let hexdump = if options.hex {
      hexdump_buffers(page, &buffers, options.hex_limit)
    } else {
      Vec::new()
    };

    let raw = if options.raw {
      decoded.as_ref().map(layout::raw)
    } else {
      None
    };

    Ok(Self {
      index: page_index,
      column: column_meta.index,
      page: page.index,
      start: page.start(),
      end: page.end(),
      bytes: page.total_bytes(),
      rows: page.num_rows,
      priority: page.priority,
      encoding: page.encoding.kind().to_string(),
      type_url: page.encoding.type_url().map(str::to_string),
      layout,
      buffers: page_buffers,
      internals,
      hexdump,
      raw,
      warnings,
    })
  }
}

impl Report for ShowReport {
  fn render_text(&self, style: Style) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", style.heading("Summary"));
    kv(&mut out, "Index", &self.index.to_string());
    kv(&mut out, "Column", &self.column.to_string());
    kv(&mut out, "Page index in column", &self.page.to_string());
    kv(&mut out, "Start", &self.start.to_string());
    kv(&mut out, "End", &self.end.to_string());
    kv(&mut out, "Bytes", &format_bytes(self.bytes));
    kv(&mut out, "Rows", &self.rows.to_string());
    kv(&mut out, "Priority", &self.priority.to_string());
    kv(&mut out, "Encoding", &self.encoding);
    if let Some(type_url) = &self.type_url {
      kv(&mut out, "Type URL", type_url);
    }

    if let Some(layout) = &self.layout {
      out.push('\n');
      let _ = writeln!(
        out,
        "{}",
        style.heading(&format!("Layout: {}", layout.kind))
      );
      for field in &layout.fields {
        let _ = writeln!(out, "  {:<26}  {}", field.name, field.value);
      }
    }

    if !self.buffers.is_empty() {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Page buffers"));
      let mut table =
        Table::new(&["buffer", "role", "start", "end", "size"]).right_align(&[0, 2, 3, 4]);
      for buffer in &self.buffers {
        table.push(vec![
          buffer.index.to_string(),
          buffer.role.clone(),
          buffer.start.to_string(),
          buffer.end.to_string(),
          format_bytes(buffer.size),
        ]);
      }
      out.push_str(&table.render(style));
    }

    match &self.internals {
      Some(PageInternals::MiniBlock { chunks }) => {
        out.push('\n');
        let _ = writeln!(
          out,
          "{}",
          style.heading(&format!(
            "Mini-block chunks ({} chunks, {} items, {} value buffer(s))",
            chunks.num_chunks, chunks.num_items, chunks.num_buffers
          ))
        );
        let mut table = Table::new(&[
          "chunk",
          "offset",
          "start",
          "bytes",
          "values",
          "levels",
          "rep",
          "def",
          "header",
          "value bufs",
          "rep index",
        ])
        .right_align(&[0, 1, 2, 3, 4, 5, 6, 7, 8]);
        for chunk in &chunks.chunks {
          table.push(vec![
            chunk.index.to_string(),
            chunk.offset.to_string(),
            chunk.start.to_string(),
            chunk.bytes.to_string(),
            chunk.values.to_string(),
            chunk.num_levels.to_string(),
            chunk
              .rep_buffer
              .map(|span| span[1].to_string())
              .unwrap_or_else(|| "-".to_string()),
            chunk
              .def_buffer
              .map(|span| span[1].to_string())
              .unwrap_or_else(|| "-".to_string()),
            chunk.header_bytes.to_string(),
            chunk
              .value_buffers
              .iter()
              .map(|span| span[1].to_string())
              .collect::<Vec<_>>()
              .join(","),
            chunk
              .rep_index
              .as_ref()
              .map(|words| {
                words
                  .iter()
                  .map(|word| word.to_string())
                  .collect::<Vec<_>>()
                  .join(",")
              })
              .unwrap_or_else(|| "-".to_string()),
          ]);
        }
        out.push_str(&table.render(style));
        render_warnings(&mut out, style, &chunks.warnings);
      }
      Some(PageInternals::FullZip {
        rep_index: Some(rep_index),
      }) => {
        out.push('\n');
        let _ = writeln!(
          out,
          "{}",
          style.heading(&format!(
            "Full-zip repetition index ({} entries, {}-byte entries)",
            rep_index.num_entries, rep_index.entry_width
          ))
        );
        let shown = rep_index.values.len().min(32);
        let _ = writeln!(out, "{:?}", &rep_index.values[..shown]);
        if shown < rep_index.values.len() {
          let _ = writeln!(out, "... {} more entry(s)", rep_index.values.len() - shown);
        }
      }
      Some(PageInternals::FullZip { rep_index: None }) | None => {}
    }

    if !self.hexdump.is_empty() {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Hexdump"));
      for section in &self.hexdump {
        out.push_str(section);
      }
    }

    if let Some(raw) = &self.raw {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Raw encoding"));
      out.push_str(raw);
      out.push('\n');
    }

    render_warnings(&mut out, style, &self.warnings);
    out
  }
}

fn kv(out: &mut String, key: &str, value: &str) {
  let _ = writeln!(out, "{key:>24}: {value}");
}

fn read_buffers(source: &mut FileSource, page: &PageMeta) -> Result<Vec<Vec<u8>>> {
  page
    .buffers
    .iter()
    .enumerate()
    .map(|(index, buffer)| {
      source
        .read_at(buffer.position, buffer.size as usize)
        .map_err(|error| anyhow::anyhow!("failed to read page buffer {index}: {error}"))
    })
    .collect()
}

fn chunk_table_report(table: miniblock::ChunkTable) -> ChunkTableReport {
  let total_values = table.total_values();
  ChunkTableReport {
    entry_width: table.entry_width,
    num_buffers: table.num_buffers,
    num_chunks: table.chunks.len(),
    num_items: table.num_items,
    total_values,
    chunks: table
      .chunks
      .into_iter()
      .map(|chunk| ChunkReport {
        index: chunk.index,
        offset: chunk.offset,
        start: chunk.start,
        metadata_word: chunk.metadata.word,
        bytes: chunk.metadata.bytes(),
        values: chunk.metadata.values,
        log2_values: chunk.metadata.log2_values(),
        num_levels: chunk.num_levels,
        rep_buffer: chunk.rep_buffer.map(|span| [span.offset, span.bytes]),
        def_buffer: chunk.def_buffer.map(|span| [span.offset, span.bytes]),
        header_bytes: chunk.header_bytes,
        value_buffers: chunk
          .value_buffers
          .into_iter()
          .map(|span| [span.offset, span.bytes])
          .collect(),
        rep_index: chunk.rep_index,
      })
      .collect(),
    warnings: table.warnings,
  }
}

fn try_full_zip_rep_index(
  layout: &pb21::FullZipLayout,
  buffers: &[Vec<u8>],
  num_rows: u64,
  warnings: &mut Vec<String>,
) -> Option<RepIndexReport> {
  let has_rep_index = match layout.details.as_ref() {
    Some(pb21::full_zip_layout::Details::BitsPerValue(_)) => layout.bits_rep > 0,
    Some(pb21::full_zip_layout::Details::BitsPerOffset(_)) => true,
    None => false,
  };
  if !has_rep_index {
    return None;
  }
  let Some(bytes) = buffers.get(1) else {
    warnings.push("full-zip page declares a repetition index but has no buffer 1".to_string());
    return None;
  };
  match miniblock::parse_full_zip_rep_index(bytes, num_rows) {
    Ok((width, values)) => Some(RepIndexReport {
      entry_width: width,
      num_entries: values.len(),
      values,
    }),
    Err(error) => {
      warnings.push(format!(
        "failed to decode full-zip repetition index: {error}"
      ));
      None
    }
  }
}

fn hexdump_buffers(page: &PageMeta, buffers: &[Vec<u8>], limit: usize) -> Vec<String> {
  let mut sections = Vec::new();
  for (index, (buffer, bytes)) in page.buffers.iter().zip(buffers.iter()).enumerate() {
    let shown = limit.min(bytes.len());
    sections.push(format!(
      "buffer {index} @ {} ({} bytes)\n{}",
      buffer.position,
      buffer.size,
      crate::view::hexdump(&bytes[..shown], buffer.position, 16)
    ));
    if shown < bytes.len() {
      sections.push(format!(
        "  ... {} more byte(s) not shown (use --hex-limit)\n",
        bytes.len() - shown
      ));
    }
  }
  sections
}

fn buffer_roles(decoded: Option<&DecodedLayout>, count: usize) -> Vec<String> {
  let mut roles: Vec<String> = match decoded {
    Some(DecodedLayout::Structural(layout)) => structural_roles(layout),
    Some(DecodedLayout::Legacy(_)) => vec!["legacy array buffer".to_string()],
    None => Vec::new(),
  };
  while roles.len() < count {
    roles.push(format!("buffer {}", roles.len()));
  }
  roles.truncate(count);
  roles
}

fn structural_roles(layout: &pb21::PageLayout) -> Vec<String> {
  match layout.layout.as_ref() {
    Some(pb21::page_layout::Layout::MiniBlockLayout(mini)) => {
      let mut roles = vec!["chunk metadata".to_string(), "chunk data".to_string()];
      if mini.dictionary.is_some() {
        roles.push("dictionary".to_string());
      }
      if mini.repetition_index_depth > 0 {
        roles.push("repetition index".to_string());
      }
      roles
    }
    Some(pb21::page_layout::Layout::FullZipLayout(full_zip)) => {
      let mut roles = vec!["zipped values".to_string()];
      if full_zip_has_rep_index(full_zip) {
        roles.push("repetition index".to_string());
      }
      roles
    }
    Some(pb21::page_layout::Layout::SparseLayout(_)) => vec!["chunk metadata".to_string()],
    Some(pb21::page_layout::Layout::BlobLayout(blob)) => blob
      .inner_layout
      .as_ref()
      .map(|inner| structural_roles(inner))
      .unwrap_or_default(),
    Some(pb21::page_layout::Layout::ConstantLayout(_)) | None => Vec::new(),
  }
}

fn full_zip_has_rep_index(layout: &pb21::FullZipLayout) -> bool {
  match layout.details.as_ref() {
    Some(pb21::full_zip_layout::Details::BitsPerValue(_)) => layout.bits_rep > 0,
    Some(pb21::full_zip_layout::Details::BitsPerOffset(_)) => true,
    None => false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn serializes_internals_as_tagged_enum() {
    let json = serde_json::to_value(PageInternals::FullZip { rep_index: None }).unwrap();
    assert_eq!(json["kind"], "full-zip");
    assert!(json["rep_index"].is_null());
  }
}
