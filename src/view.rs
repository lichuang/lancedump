// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Small text-rendering helpers.

use std::fmt::Write;

/// ANSI styling for the text renderer.
///
/// Colors are only emitted when the target is a terminal (or `--color always`),
/// so piped output and tests stay plain.
#[derive(Debug, Clone, Copy)]
pub struct Style {
  color: bool,
}

impl Style {
  pub fn new(color: bool) -> Self {
    Self { color }
  }

  pub fn heading(&self, text: &str) -> String {
    if self.color {
      format!("\x1b[1;36m{text}\x1b[0m")
    } else {
      text.to_string()
    }
  }

  pub fn warning(&self, text: &str) -> String {
    if self.color {
      format!("\x1b[33m{text}\x1b[0m")
    } else {
      text.to_string()
    }
  }
}

/// A simple aligned table.
pub struct Table {
  headers: Vec<String>,
  rows: Vec<Vec<String>>,
  right_align: Vec<bool>,
}

impl Table {
  pub fn new(headers: &[&str]) -> Self {
    Self {
      headers: headers.iter().map(|header| header.to_string()).collect(),
      rows: Vec::new(),
      right_align: vec![false; headers.len()],
    }
  }

  /// Right-align the given column indices (usually numeric columns).
  pub fn right_align(mut self, columns: &[usize]) -> Self {
    for &column in columns {
      if let Some(slot) = self.right_align.get_mut(column) {
        *slot = true;
      }
    }
    self
  }

  pub fn push(&mut self, row: Vec<String>) {
    debug_assert_eq!(row.len(), self.headers.len());
    self.rows.push(row);
  }

  pub fn render(&self) -> String {
    let mut widths: Vec<usize> = self
      .headers
      .iter()
      .map(|header| header.chars().count())
      .collect();
    for row in &self.rows {
      for (index, cell) in row.iter().enumerate() {
        widths[index] = widths[index].max(cell.chars().count());
      }
    }

    let mut out = String::new();
    write_row(&mut out, &self.headers, &widths, &self.right_align);
    let separator: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    write_row(&mut out, &separator, &widths, &self.right_align);
    for row in &self.rows {
      write_row(&mut out, row, &widths, &self.right_align);
    }
    out
  }
}

fn write_row(out: &mut String, cells: &[String], widths: &[usize], right: &[bool]) {
  for (index, cell) in cells.iter().enumerate() {
    if index > 0 {
      out.push_str("  ");
    }
    let padding = widths[index].saturating_sub(cell.chars().count());
    let last = index + 1 == cells.len();
    if right[index] {
      let _ = write!(out, "{}{}", " ".repeat(padding), cell);
    } else if last {
      // Do not pad the final column: avoids trailing whitespace.
      out.push_str(cell);
    } else {
      let _ = write!(out, "{}{}", cell, " ".repeat(padding));
    }
  }
  out.push('\n');
}

/// Human-readable byte count.
pub fn format_bytes(bytes: u64) -> String {
  const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
  let mut value = bytes as f64;
  let mut unit = 0;
  while value >= 1024.0 && unit + 1 < UNITS.len() {
    value /= 1024.0;
    unit += 1;
  }
  if unit == 0 {
    format!("{bytes} B")
  } else {
    format!("{value:.1} {}", UNITS[unit])
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn formats_bytes() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1023), "1023 B");
    assert_eq!(format_bytes(1024), "1.0 KiB");
    assert_eq!(format_bytes(1536), "1.5 KiB");
    assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
  }

  #[test]
  fn renders_table() {
    let mut table = Table::new(&["a", "bb"]);
    table = table.right_align(&[1]);
    table.push(vec!["x".to_string(), "1".to_string()]);
    table.push(vec!["long".to_string(), "22".to_string()]);
    let rendered = table.render();
    assert!(rendered.contains("a     bb"));
    assert!(rendered.contains("long  22"));
  }
}
