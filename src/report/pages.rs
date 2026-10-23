// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::collections::HashSet;

use anyhow::Result;
use serde::Serialize;

use crate::format::container::FileMeta;
use crate::format::layout;
use crate::report::Report;
use crate::view::{Style, Table, format_bytes, layout_color, render_warnings};

#[derive(Debug, Serialize)]
pub struct PageRow {
  /// Sequential index in this listing (file order: column, then page).
  pub global_index: u32,
  pub column: u32,
  pub page: u32,
  pub start: u64,
  pub end: u64,
  pub bytes: u64,
  pub rows: u64,
  pub priority: u64,
  pub buffers: usize,
  pub layout: String,
  pub detail: String,
  /// Raw `Any` type URL when present, useful for unknown layouts.
  pub type_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PagesReport {
  pub pages: Vec<PageRow>,
  pub warnings: Vec<String>,
}

impl PagesReport {
  pub fn from_meta(meta: &FileMeta, column: Option<u32>) -> Result<Self> {
    let mut warnings = meta.warnings.clone();
    let selected: HashSet<u32> = meta
      .selected_columns(column)?
      .into_iter()
      .map(|column| column.index)
      .collect();

    // Physical order; the global index is assigned before filtering, so
    // `--column N` keeps each page's number stable.
    let entries = meta.pages_in_physical_order();
    let mut pages = Vec::with_capacity(entries.len());
    for (global_index, (column, page)) in entries.into_iter().enumerate() {
      if !selected.contains(&column.index) {
        continue;
      }
      let (layout, detail) = match layout::decode_layout(&page.encoding) {
        Ok(Some(decoded)) => {
          let summary = layout::summary(&decoded);
          (summary.kind.to_string(), summary.detail)
        }
        Ok(None) => (page.encoding.kind().to_string(), String::new()),
        Err(error) => {
          warnings.push(format!(
            "column {} page {}: {error:#}",
            column.index, page.index
          ));
          ("undecodable".to_string(), String::new())
        }
      };
      pages.push(PageRow {
        global_index: global_index as u32,
        column: column.index,
        page: page.index,
        start: page.start(),
        end: page.end(),
        bytes: page.total_bytes(),
        rows: page.num_rows,
        priority: page.priority,
        buffers: page.buffers.len(),
        layout,
        detail,
        type_url: page.encoding.type_url().map(str::to_string),
      });
    }

    Ok(Self { pages, warnings })
  }
}

impl Report for PagesReport {
  fn render_text(&self, style: Style) -> String {
    let mut out = String::new();
    let mut table = Table::new(&[
      "#",
      "column",
      "page (in column)",
      "start",
      "end",
      "bytes",
      "rows",
      "priority",
      "buffers",
      "layout",
    ])
    .right_align(&[0, 1, 2, 3, 4, 5, 6, 7, 8]);
    for page in &self.pages {
      let layout = if page.detail.is_empty() {
        page.layout.clone()
      } else {
        format!("{} ({})", page.layout, page.detail)
      };
      let layout_column = 9;
      let colors: Vec<(usize, crate::view::Color)> = layout_color(&page.layout)
        .map(|color| (layout_column, color))
        .into_iter()
        .collect();
      table.push_colored(
        vec![
          page.global_index.to_string(),
          page.column.to_string(),
          page.page.to_string(),
          page.start.to_string(),
          page.end.to_string(),
          format_bytes(page.bytes),
          page.rows.to_string(),
          page.priority.to_string(),
          page.buffers.to_string(),
          layout,
        ],
        &colors,
      );
    }
    out.push_str(&table.render(style));
    render_warnings(&mut out, style, &self.warnings);
    out
  }
}
