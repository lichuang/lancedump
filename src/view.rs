// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Small text-rendering helpers.

use std::fmt::Write;

/// A color used for a table cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
  Green,
  Yellow,
  Blue,
  Magenta,
  Cyan,
  Red,
  Dim,
}

impl Color {
  fn code(self) -> &'static str {
    match self {
      Self::Green => "32",
      Self::Yellow => "33",
      Self::Blue => "34",
      Self::Magenta => "35",
      Self::Cyan => "36",
      Self::Red => "31",
      Self::Dim => "2",
    }
  }
}

/// ANSI styling for the text renderer.
///
/// Colors are only emitted when the target is a terminal (or
/// `CLICOLOR_FORCE` is set), so piped output and tests stay plain.
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

  pub fn paint(&self, text: &str, color: Color) -> String {
    if self.color {
      format!("\x1b[{}m{text}\x1b[0m", color.code())
    } else {
      text.to_string()
    }
  }
}

/// A color for a page layout kind.
pub fn layout_color(kind: &str) -> Option<Color> {
  match kind {
    "mini-block" => Some(Color::Green),
    "full-zip" => Some(Color::Yellow),
    "sparse" => Some(Color::Magenta),
    "constant" => Some(Color::Blue),
    "blob" => Some(Color::Cyan),
    "legacy-v2.0" => Some(Color::Dim),
    "undecodable" => Some(Color::Red),
    _ => None,
  }
}

/// A simple aligned table.
pub struct Table {
  headers: Vec<String>,
  rows: Vec<Vec<String>>,
  cell_colors: Vec<Vec<Option<Color>>>,
  right_align: Vec<bool>,
}

impl Table {
  pub fn new(headers: &[&str]) -> Self {
    Self {
      headers: headers.iter().map(|header| header.to_string()).collect(),
      rows: Vec::new(),
      cell_colors: Vec::new(),
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
    self.push_colored(row, &[]);
  }

  /// Push a row, coloring the given `(column, color)` cells.
  pub fn push_colored(&mut self, row: Vec<String>, colored: &[(usize, Color)]) {
    debug_assert_eq!(row.len(), self.headers.len());
    let mut colors = vec![None; row.len()];
    for &(column, color) in colored {
      if let Some(slot) = colors.get_mut(column) {
        *slot = Some(color);
      }
    }
    self.rows.push(row);
    self.cell_colors.push(colors);
  }

  pub fn render(&self, style: Style) -> String {
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
    write_row(
      &mut out,
      &self.headers,
      &widths,
      &self.right_align,
      &[],
      style,
    );
    let separator: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    write_row(&mut out, &separator, &widths, &self.right_align, &[], style);
    for (row, colors) in self.rows.iter().zip(&self.cell_colors) {
      write_row(&mut out, row, &widths, &self.right_align, colors, style);
    }
    out
  }
}

fn write_row(
  out: &mut String,
  cells: &[String],
  widths: &[usize],
  right: &[bool],
  colors: &[Option<Color>],
  style: Style,
) {
  for (index, cell) in cells.iter().enumerate() {
    if index > 0 {
      out.push_str("  ");
    }
    let padding = widths[index].saturating_sub(cell.chars().count());
    let last = index + 1 == cells.len();
    let rendered = if right[index] {
      format!("{}{}", " ".repeat(padding), cell)
    } else if last {
      // Do not pad the final column: avoids trailing whitespace.
      cell.clone()
    } else {
      format!("{}{}", cell, " ".repeat(padding))
    };
    match colors.get(index).copied().flatten() {
      Some(color) => out.push_str(&style.paint(&rendered, color)),
      None => out.push_str(&rendered),
    }
  }
  out.push('\n');
}

/// Hex dump with an ASCII gutter.
pub fn hexdump(bytes: &[u8], base_offset: u64, width: usize) -> String {
  let width = width.clamp(1, 64);
  let mut out = String::new();
  for (line, chunk) in bytes.chunks(width).enumerate() {
    let offset = base_offset + (line * width) as u64;
    let _ = write!(out, "{offset:08x}  ");
    for i in 0..width {
      match chunk.get(i) {
        Some(byte) => {
          let _ = write!(out, "{byte:02x} ");
        }
        None => out.push_str("   "),
      }
      if i + 1 == width / 2 {
        out.push(' ');
      }
    }
    out.push_str(" |");
    for byte in chunk {
      out.push(if byte.is_ascii_graphic() || *byte == b' ' {
        *byte as char
      } else {
        '.'
      });
    }
    out.push_str("|\n");
  }
  out
}

/// Render a `Warnings` section, if there are any warnings.
pub fn render_warnings(out: &mut String, style: Style, warnings: &[String]) {
  if warnings.is_empty() {
    return;
  }
  out.push('\n');
  let _ = writeln!(out, "{}", style.heading("Warnings"));
  for warning in warnings {
    let _ = writeln!(out, "{}", style.warning(&format!("  - {warning}")));
  }
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
    let rendered = table.render(Style::new(false));
    assert!(rendered.contains("a     bb"));
    assert!(rendered.contains("long  22"));
  }

  #[test]
  fn maps_layout_colors() {
    assert_eq!(layout_color("mini-block"), Some(Color::Green));
    assert_eq!(layout_color("full-zip"), Some(Color::Yellow));
    assert_eq!(layout_color("sparse"), Some(Color::Magenta));
    assert_eq!(layout_color("constant"), Some(Color::Blue));
    assert_eq!(layout_color("blob"), Some(Color::Cyan));
    assert_eq!(layout_color("legacy-v2.0"), Some(Color::Dim));
    assert_eq!(layout_color("unknown"), None);
  }

  #[test]
  fn colors_table_cells() {
    let mut table = Table::new(&["layout"]);
    table.push_colored(vec!["mini-block".to_string()], &[(0, Color::Green)]);
    let colored = table.render(Style::new(true));
    assert!(colored.contains("\x1b[32mmini-block\x1b[0m"));

    let plain = table.render(Style::new(false));
    assert!(!plain.contains("\x1b["));
  }
}
