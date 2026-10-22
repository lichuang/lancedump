// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::fmt::Write;

use serde::Serialize;

use crate::format::container::FileMeta;
use crate::format::footer::FooterLayout;
use crate::proto::pb;
use crate::report::Report;
use crate::view::{Style, Table, format_bytes};

#[derive(Debug, Serialize)]
pub struct FormatReport {
  pub major: u16,
  pub minor: u16,
  pub name: String,
}

#[derive(Debug, Serialize)]
pub struct SchemaSummary {
  pub fields: usize,
}

#[derive(Debug, Serialize)]
pub struct FooterReport {
  /// "v2", "legacy", or "unknown".
  pub layout: String,
  pub column_meta_start: Option<u64>,
  pub column_meta_offsets_start: Option<u64>,
  pub global_buff_offsets_start: Option<u64>,
  pub num_global_buffers: Option<u32>,
  pub num_columns: Option<u32>,
  /// Only present for legacy v1 files.
  pub metadata_position: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct RegionReport {
  pub name: String,
  pub start: u64,
  pub end: u64,
  pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct BufferReport {
  pub index: u32,
  pub position: u64,
  pub size: u64,
  /// What this buffer is, derived from schema metadata pointers.
  pub description: String,
}

#[derive(Debug, Serialize)]
pub struct CmoEntry {
  pub column: u32,
  /// Absolute position and size of the column's `ColumnMetadata` record.
  pub position: u64,
  pub size: u64,
  pub pages: usize,
  pub rows: u64,
  /// Page buffers plus column-level buffers.
  pub buffers: usize,
  /// Sum of page and column buffer sizes, excluding alignment padding.
  pub bytes: u64,
  pub undecodable: bool,
}

#[derive(Debug, Serialize)]
pub struct InfoReport {
  pub path: String,
  pub file_size: u64,
  pub format: FormatReport,
  pub num_rows: Option<u64>,
  pub num_columns: Option<u32>,
  pub num_global_buffers: Option<u32>,
  pub schema: Option<SchemaSummary>,
  pub footer: FooterReport,
  pub regions: Vec<RegionReport>,
  pub cmo: Vec<CmoEntry>,
  pub global_buffers: Vec<BufferReport>,
  pub warnings: Vec<String>,
}

impl InfoReport {
  pub fn from_meta(meta: &FileMeta) -> Self {
    let format = FormatReport {
      major: meta.footer.major_version,
      minor: meta.footer.minor_version,
      name: meta.version.to_string(),
    };

    let footer = match meta.footer.layout {
      FooterLayout::V2(v2) => FooterReport {
        layout: "v2".to_string(),
        column_meta_start: Some(v2.column_meta_start),
        column_meta_offsets_start: Some(v2.column_meta_offsets_start),
        global_buff_offsets_start: Some(v2.global_buff_offsets_start),
        num_global_buffers: Some(v2.num_global_buffers),
        num_columns: Some(v2.num_columns),
        metadata_position: None,
      },
      FooterLayout::Legacy { metadata_position } => FooterReport {
        layout: "legacy".to_string(),
        column_meta_start: None,
        column_meta_offsets_start: None,
        global_buff_offsets_start: None,
        num_global_buffers: None,
        num_columns: None,
        metadata_position: Some(metadata_position),
      },
      FooterLayout::Unknown => FooterReport {
        layout: "unknown".to_string(),
        column_meta_start: None,
        column_meta_offsets_start: None,
        global_buff_offsets_start: None,
        num_global_buffers: None,
        num_columns: None,
        metadata_position: None,
      },
    };

    let schema = meta.schema.as_ref().map(|schema| SchemaSummary {
      fields: schema.fields.len(),
    });

    let regions = meta
      .regions()
      .unwrap_or_default()
      .into_iter()
      .map(|region| RegionReport {
        name: region.name.to_string(),
        start: region.start,
        end: region.end,
        size: region.size(),
      })
      .collect();

    let cmo = meta
      .column_metadata_table
      .iter()
      .enumerate()
      .map(|(index, entry)| {
        let column = meta.columns.get(index);
        CmoEntry {
          column: column.map(|column| column.index).unwrap_or(index as u32),
          position: entry.position,
          size: entry.size,
          pages: column.map(|column| column.pages.len()).unwrap_or(0),
          rows: column.map(|column| column.total_rows()).unwrap_or(0),
          buffers: column
            .map(|column| {
              column
                .pages
                .iter()
                .map(|page| page.buffers.len())
                .sum::<usize>()
                + column.column_buffers.len()
            })
            .unwrap_or(0),
          bytes: column.map(|column| column.total_bytes()).unwrap_or(0),
          undecodable: column.map(|column| column.undecodable).unwrap_or(true),
        }
      })
      .collect();

    let global_buffers = meta
      .global_buffers
      .iter()
      .enumerate()
      .map(|(index, buffer)| BufferReport {
        index: index as u32,
        position: buffer.position,
        size: buffer.size,
        description: buffer_description(index as u32, meta.schema.as_ref(), meta.num_rows),
      })
      .collect();

    Self {
      path: meta.path.display().to_string(),
      file_size: meta.file_size,
      format,
      num_rows: meta.num_rows,
      num_columns: meta.num_columns(),
      num_global_buffers: meta.num_global_buffers(),
      schema,
      footer,
      regions,
      cmo,
      global_buffers,
      warnings: meta.warnings.clone(),
    }
  }
}

impl Report for InfoReport {
  fn render_text(&self, style: Style) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", style.heading("Summary"));
    kv(&mut out, "Path", &self.path);
    kv(&mut out, "Size", &format_bytes(self.file_size));
    kv(
      &mut out,
      "Format",
      &format!(
        "{} ({}.{})",
        self.format.name, self.format.major, self.format.minor
      ),
    );
    kv(&mut out, "Rows", &optional_number(self.num_rows));
    kv(&mut out, "Columns", &optional_number(self.num_columns));
    kv(
      &mut out,
      "Global buffers",
      &optional_number(self.num_global_buffers),
    );
    match &self.schema {
      Some(schema) => kv(&mut out, "Schema", &format!("{} field(s)", schema.fields)),
      None => kv(&mut out, "Schema", "<unavailable>"),
    }
    out.push('\n');
    let _ = writeln!(out, "{}", style.heading("Footer"));
    raw_kv(&mut out, "layout", &self.footer.layout);
    if let Some(value) = self.footer.column_meta_start {
      raw_kv(&mut out, "column_meta_start", &value.to_string());
    }
    if let Some(value) = self.footer.column_meta_offsets_start {
      raw_kv(&mut out, "column_meta_offsets_start", &value.to_string());
    }
    if let Some(value) = self.footer.global_buff_offsets_start {
      raw_kv(&mut out, "global_buff_offsets_start", &value.to_string());
    }
    if let Some(value) = self.footer.num_global_buffers {
      raw_kv(&mut out, "num_global_buffers", &value.to_string());
    }
    if let Some(value) = self.footer.num_columns {
      raw_kv(&mut out, "num_columns", &value.to_string());
    }
    if let Some(value) = self.footer.metadata_position {
      raw_kv(&mut out, "metadata_position", &value.to_string());
    }

    if !self.regions.is_empty() {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Regions"));
      let mut table = Table::new(&["name", "start", "end", "size"]).right_align(&[1, 2, 3]);
      for region in &self.regions {
        table.push(vec![
          region.name.clone(),
          region.start.to_string(),
          region.end.to_string(),
          format_bytes(region.size),
        ]);
      }
      out.push_str(&table.render());
    }

    if !self.cmo.is_empty() {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Column Metadata Offset Table"));
      let mut table = Table::new(&[
        "column",
        "position",
        "meta size",
        "pages",
        "rows",
        "buffers",
        "buf bytes",
      ])
      .right_align(&[0, 1, 2, 3, 4, 5, 6]);
      for entry in &self.cmo {
        if entry.undecodable {
          table.push(vec![
            entry.column.to_string(),
            entry.position.to_string(),
            format_bytes(entry.size),
            "?".to_string(),
            "?".to_string(),
            "?".to_string(),
            "?".to_string(),
          ]);
        } else {
          table.push(vec![
            entry.column.to_string(),
            entry.position.to_string(),
            format_bytes(entry.size),
            entry.pages.to_string(),
            entry.rows.to_string(),
            entry.buffers.to_string(),
            format_bytes(entry.bytes),
          ]);
        }
      }
      out.push_str(&table.render());
    }

    if !self.global_buffers.is_empty() {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Global Buffer Offset Table"));
      let mut table =
        Table::new(&["index", "position", "size", "description"]).right_align(&[0, 1, 2]);
      for buffer in &self.global_buffers {
        table.push(vec![
          buffer.index.to_string(),
          buffer.position.to_string(),
          format_bytes(buffer.size),
          buffer.description.clone(),
        ]);
      }
      out.push_str(&table.render());
    }

    if !self.warnings.is_empty() {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Warnings"));
      for warning in &self.warnings {
        let _ = writeln!(out, "{}", style.warning(&format!("  - {warning}")));
      }
    }

    out
  }
}

/// Describe a global buffer from the pointers Lance stores in schema metadata.
///
/// `GBO[0]` is always the schema. Later buffers are opaque bytes; index data
/// written into a data file records its buffer index in schema metadata, either
/// as a plain number (`lance:ivf`) or as `<index>:<extra>` (`lance.seed.*`).
fn buffer_description(index: u32, schema: Option<&pb::Schema>, num_rows: Option<u64>) -> String {
  if index == 0 {
    return match schema {
      Some(schema) => format!(
        "schema (FileDescriptor: {} field(s), {} row(s))",
        schema.fields.len(),
        num_rows.unwrap_or(0)
      ),
      None => "schema (FileDescriptor)".to_string(),
    };
  }

  let Some(schema) = schema else {
    return "unknown".to_string();
  };

  let mut references = Vec::new();
  for (key, value) in &schema.metadata {
    let text = String::from_utf8_lossy(value);
    let head = text.split(':').next().unwrap_or_default().trim();
    if let Ok(parsed) = head.parse::<u64>()
      && parsed == index as u64
    {
      references.push(describe_reference(key, &text));
    }
  }

  if references.is_empty() {
    "unreferenced (content unknown)".to_string()
  } else {
    references.join("; ")
  }
}

fn describe_reference(key: &str, value: &str) -> String {
  let kind = if key == "lance:ivf" {
    "IVF metadata"
  } else if key == "lance:index" {
    "index metadata"
  } else if key == "lance:sq" {
    "scalar quantization metadata"
  } else if key == "storage_metadata" {
    "index storage metadata"
  } else if key.starts_with("lance.seed.") {
    "index seed"
  } else {
    "schema metadata"
  };
  format!("{kind} ({key}={value})")
}

fn kv(out: &mut String, key: &str, value: &str) {
  let _ = writeln!(out, "{key:>18}: {value}");
}

fn raw_kv(out: &mut String, key: &str, value: &str) {
  let _ = writeln!(out, "  {key:<26}  {value}");
}

fn optional_number<T: ToString>(value: Option<T>) -> String {
  value
    .map(|value| value.to_string())
    .unwrap_or_else(|| "<unknown>".to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn describes_global_buffers() {
    let mut schema = pb::Schema::default();
    schema
      .metadata
      .insert("lance:ivf".to_string(), b"2".to_vec());
    schema
      .metadata
      .insert("lance.seed.captions".to_string(), b"3:8192".to_vec());

    assert!(
      buffer_description(0, Some(&schema), Some(12))
        .contains("schema (FileDescriptor: 0 field(s), 12 row(s))")
    );
    assert_eq!(
      buffer_description(2, Some(&schema), Some(12)),
      "IVF metadata (lance:ivf=2)"
    );
    assert_eq!(
      buffer_description(3, Some(&schema), Some(12)),
      "index seed (lance.seed.captions=3:8192)"
    );
    assert_eq!(
      buffer_description(4, Some(&schema), Some(12)),
      "unreferenced (content unknown)"
    );
  }
}
