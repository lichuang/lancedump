// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::fmt::Write;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::format::container::{FileMeta, PageMeta};
use crate::format::decode;
use crate::format::fullzip;
use crate::format::layout::{self, DecodedLayout};
use crate::format::miniblock;
use crate::format::miniblock::ChunkSpan;
use crate::proto::pb21;
use crate::report::Report;
use crate::source::FileSource;
use crate::view::{Style, Table, format_bytes, render_warnings};

#[derive(Debug, Clone, Copy)]
pub struct ShowOptions {
  pub hex: bool,
  pub hex_limit: usize,
  pub raw: bool,
  pub detail: bool,
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
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rep_levels: Option<Vec<u16>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub def_levels: Option<Vec<u16>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub decoded_values: Option<DecodedValuesReport>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub value_buffer_sizes: Option<Vec<u64>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DecodedValuesReport {
  Fixed {
    bits_per_value: u64,
    items_per_value: u64,
    values: Vec<u64>,
  },
  Variable {
    bits_per_offset: u64,
    values: Vec<String>,
    sizes: Vec<u64>,
  },
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
  MiniBlock {
    chunks: ChunkTableReport,
  },
  FullZip {
    rep_index: Option<RepIndexReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<FullZipDetailReport>,
  },
}

#[derive(Debug, Serialize)]
pub struct FullZipDetailReport {
  pub rep_levels: Vec<u16>,
  pub def_levels: Vec<u16>,
  pub item_sizes: Vec<u64>,
  pub decoded_values: DecodedValuesReport,
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

    let data = buffers
      .get(1)
      .map(|buffer| buffer.as_slice())
      .unwrap_or(&[]);
    let internals = match decoded.as_ref() {
      Some(DecodedLayout::Structural(layout)) => match layout.layout.as_ref() {
        Some(pb21::page_layout::Layout::MiniBlockLayout(mini)) => {
          match miniblock::parse(mini, page, &buffers, &mut warnings) {
            Ok(table) => Some(PageInternals::MiniBlock {
              chunks: chunk_table_report(table, mini, data, options.detail, &mut warnings),
            }),
            Err(error) => {
              warnings.push(format!("failed to decode mini-block chunk table: {error}"));
              None
            }
          }
        }
        Some(pb21::page_layout::Layout::FullZipLayout(full_zip)) => {
          let rep_index = try_full_zip_rep_index(full_zip, &buffers, page.num_rows, &mut warnings);
          let detail = if options.detail {
            match fullzip::parse_detail(full_zip, &buffers, &mut warnings) {
              Ok(detail) => Some(full_zip_detail_report(detail)),
              Err(error) => {
                warnings.push(format!("failed to decode full-zip values: {error:#}"));
                None
              }
            }
          } else {
            None
          };
          Some(PageInternals::FullZip { rep_index, detail })
        }
        _ => None,
      },
      _ => None,
    };

    if options.detail {
      let kind = decoded
        .as_ref()
        .map(|decoded| layout::summary(decoded).kind)
        .unwrap_or("unknown");
      if !matches!(kind, "mini-block" | "full-zip") {
        warnings.push(format!(
          "detail arrays are only supported for mini-block and full-zip pages (layout: {kind})"
        ));
      }
    }

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

        if chunks.chunks.iter().any(|chunk| {
          chunk.rep_levels.is_some() || chunk.def_levels.is_some() || chunk.decoded_values.is_some()
        }) {
          out.push('\n');
          let _ = writeln!(out, "{}", style.heading("Chunk details"));
          for chunk in &chunks.chunks {
            if chunk.rep_levels.is_none()
              && chunk.def_levels.is_none()
              && chunk.decoded_values.is_none()
            {
              continue;
            }
            let _ = writeln!(out, "chunk {}:", chunk.index);
            if let Some(levels) = &chunk.rep_levels {
              push_array(&mut out, &format!("rep levels ({})", levels.len()), levels);
            }
            if let Some(levels) = &chunk.def_levels {
              push_array(&mut out, &format!("def levels ({})", levels.len()), levels);
            }
            if let Some(values) = &chunk.decoded_values {
              push_decoded_values(&mut out, values);
            }
            if let Some(sizes) = &chunk.value_buffer_sizes {
              let label = if sizes.len() == 1 {
                "value buffer sizes (1 buffer)".to_string()
              } else {
                format!("value buffer sizes ({} buffers)", sizes.len())
              };
              push_array(&mut out, &label, sizes);
            }
          }
        }
      }
      Some(PageInternals::FullZip { rep_index, detail }) => {
        if let Some(rep_index) = rep_index {
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
        if let Some(detail) = detail {
          out.push('\n');
          let _ = writeln!(out, "{}", style.heading("Full-zip details"));
          push_array(
            &mut out,
            &format!("rep levels ({})", detail.rep_levels.len()),
            &detail.rep_levels,
          );
          push_array(
            &mut out,
            &format!("def levels ({})", detail.def_levels.len()),
            &detail.def_levels,
          );
          push_array(
            &mut out,
            &format!("item sizes ({})", detail.item_sizes.len()),
            &detail.item_sizes,
          );
          push_decoded_values(&mut out, &detail.decoded_values);
        }
      }
      None => {}
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

fn chunk_table_report(
  table: miniblock::ChunkTable,
  layout: &pb21::MiniBlockLayout,
  data: &[u8],
  detail: bool,
  warnings: &mut Vec<String>,
) -> ChunkTableReport {
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
      .map(|chunk| {
        let chunk_bytes = data
          .get(chunk.offset as usize..chunk.offset as usize + chunk.metadata.bytes() as usize)
          .unwrap_or(&[]);
        let (rep_levels, def_levels, decoded_values, value_buffer_sizes) = if detail {
          (
            decode_level_detail(
              layout.rep_compression.as_ref(),
              chunk.rep_buffer,
              chunk_bytes,
              chunk.num_levels,
              "rep",
              chunk.index,
              warnings,
            ),
            decode_level_detail(
              layout.def_compression.as_ref(),
              chunk.def_buffer,
              chunk_bytes,
              chunk.num_levels,
              "def",
              chunk.index,
              warnings,
            ),
            decode_value_detail(
              layout.value_compression.as_ref(),
              &chunk,
              chunk_bytes,
              warnings,
            ),
            Some(chunk.value_buffers.iter().map(|span| span.bytes).collect()),
          )
        } else {
          (None, None, None, None)
        };

        ChunkReport {
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
          rep_levels,
          def_levels,
          decoded_values,
          value_buffer_sizes,
        }
      })
      .collect(),
    warnings: table.warnings,
  }
}

fn span_bytes(data: &[u8], span: ChunkSpan) -> Option<&[u8]> {
  let start = span.offset as usize;
  data.get(start..start + span.bytes as usize)
}

#[allow(clippy::too_many_arguments)]
fn decode_level_detail(
  compression: Option<&pb21::CompressiveEncoding>,
  span: Option<ChunkSpan>,
  chunk_bytes: &[u8],
  num_levels: u16,
  label: &str,
  chunk_index: usize,
  warnings: &mut Vec<String>,
) -> Option<Vec<u16>> {
  let (Some(compression), Some(span)) = (compression, span) else {
    return None;
  };
  let Some(bytes) = span_bytes(chunk_bytes, span) else {
    warnings.push(format!(
      "chunk {chunk_index}: {label} buffer is out of bounds"
    ));
    return None;
  };
  match decode::decode_levels(compression, bytes, num_levels as u64) {
    Ok(levels) => Some(levels),
    Err(error) => {
      warnings.push(format!(
        "chunk {chunk_index}: failed to decode {label} levels: {error:#}"
      ));
      None
    }
  }
}

fn decode_value_detail(
  compression: Option<&pb21::CompressiveEncoding>,
  chunk: &miniblock::Chunk,
  chunk_bytes: &[u8],
  warnings: &mut Vec<String>,
) -> Option<DecodedValuesReport> {
  let compression = compression?;
  let mut buffers = Vec::with_capacity(chunk.value_buffers.len());
  for span in &chunk.value_buffers {
    let Some(bytes) = span_bytes(chunk_bytes, *span) else {
      warnings.push(format!(
        "chunk {}: value buffer is out of bounds",
        chunk.index
      ));
      return None;
    };
    buffers.push(bytes);
  }
  match decode::decode_values(compression, &buffers, chunk.metadata.values) {
    Ok(decode::DecodedValues::Fixed {
      bits_per_value,
      items_per_value,
      values,
    }) => Some(DecodedValuesReport::Fixed {
      bits_per_value,
      items_per_value,
      values,
    }),
    Ok(decode::DecodedValues::Variable {
      bits_per_offset,
      values,
    }) => {
      let sizes = values.iter().map(|value| value.len() as u64).collect();
      let values = values.into_iter().map(format_value_bytes).collect();
      Some(DecodedValuesReport::Variable {
        bits_per_offset,
        values,
        sizes,
      })
    }
    Err(error) => {
      warnings.push(format!(
        "chunk {}: failed to decode values: {error:#}",
        chunk.index
      ));
      None
    }
  }
}

fn push_array<T: ToString>(out: &mut String, label: &str, values: &[T]) {
  let _ = write!(out, "  {label}: [");
  for (index, value) in values.iter().enumerate() {
    if index > 0 {
      out.push_str(", ");
    }
    if index > 0 && index % 32 == 0 {
      out.push('\n');
      out.push_str("    ");
    }
    out.push_str(&value.to_string());
  }
  out.push_str("]\n");
}

fn push_decoded_values(out: &mut String, values: &DecodedValuesReport) {
  match values {
    DecodedValuesReport::Fixed {
      bits_per_value,
      items_per_value,
      values,
    } => {
      let items = (*items_per_value).max(1);
      let logical = values.len() as u64 / items;
      let label = if items > 1 {
        format!("values ({logical} x {items} items, {bits_per_value}-bit)")
      } else {
        format!("values ({logical}, {bits_per_value}-bit)")
      };
      push_array(out, &label, values);
    }
    DecodedValuesReport::Variable {
      bits_per_offset,
      values,
      sizes,
    } => {
      let _ = writeln!(
        out,
        "  values ({} values, variable, {bits_per_offset}-bit offsets):",
        values.len()
      );
      for (index, (value, size)) in values.iter().zip(sizes).enumerate() {
        let _ = writeln!(out, "    [{index}] ({size} bytes) {value}");
      }
      push_array(out, &format!("value sizes ({})", sizes.len()), sizes);
    }
  }
}

fn format_value_bytes(bytes: Vec<u8>) -> String {
  match String::from_utf8(bytes.clone()) {
    Ok(text)
      if !text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t') =>
    {
      text
    }
    _ => {
      let mut out = String::from("0x");
      for byte in bytes {
        let _ = write!(out, "{byte:02x}");
      }
      out
    }
  }
}

fn full_zip_detail_report(detail: fullzip::FullZipDetail) -> FullZipDetailReport {
  let decoded_values = match detail.values {
    fullzip::FullZipValues::Fixed {
      bits_per_value,
      values,
    } => DecodedValuesReport::Fixed {
      bits_per_value,
      items_per_value: 1,
      values,
    },
    fullzip::FullZipValues::Variable {
      bits_per_offset,
      values,
    } => {
      let sizes = values.iter().map(|value| value.len() as u64).collect();
      let values = values.into_iter().map(format_value_bytes).collect();
      DecodedValuesReport::Variable {
        bits_per_offset,
        values,
        sizes,
      }
    }
  };
  FullZipDetailReport {
    rep_levels: detail.rep_levels,
    def_levels: detail.def_levels,
    item_sizes: detail.item_sizes,
    decoded_values,
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
  match fullzip::parse_rep_index(bytes, num_rows) {
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
    let json = serde_json::to_value(PageInternals::FullZip {
      rep_index: None,
      detail: None,
    })
    .unwrap();
    assert_eq!(json["kind"], "full-zip");
    assert!(json["rep_index"].is_null());
  }
}
