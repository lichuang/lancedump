// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
  name = "ldp",
  version,
  about = "Read-only inspector for Lance data files",
  long_about = "ldp opens a single Lance data file (data/*.lance) and explains its \
                  physical structure: footer, schema, column metadata, pages, and \
                  page layouts.\n\n\
                  No dataset manifest is required.",
  disable_help_subcommand = true
)]
pub struct Cli {
  /// Output format.
  #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
  pub format: OutputFormat,

  #[command(subcommand)]
  pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
  Text,
  Json,
}

#[derive(Debug, Args)]
pub struct ColumnArgs {
  /// Path to a *.lance data file.
  #[arg(value_name = "FILE")]
  pub path: PathBuf,

  /// Restrict output to this column index.
  #[arg(long, value_name = "INDEX")]
  pub column: Option<u32>,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
  /// Path to a *.lance data file.
  #[arg(value_name = "FILE")]
  pub path: PathBuf,

  /// Global page index (the `#` column from `ldp pages`).
  #[arg(long, value_name = "INDEX")]
  pub page: u32,

  /// Include a hex dump of each page buffer.
  #[arg(long)]
  pub hex: bool,

  /// Maximum bytes to hex dump per buffer.
  #[arg(long, default_value_t = 1024, value_name = "BYTES")]
  pub hex_limit: usize,

  /// Print the decoded protobuf layout in raw debug form.
  #[arg(long)]
  pub raw: bool,
}

#[derive(Debug, Args)]
pub struct FileArgs {
  /// Path to a *.lance data file.
  #[arg(value_name = "FILE")]
  pub path: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum Command {
  /// Show the footer, regions, global buffers, and a column summary.
  Info(FileArgs),

  /// Show the schema embedded in global buffer 0.
  Schema(FileArgs),

  /// List pages: position, size, rows, priority, buffers, and layout.
  Pages(ColumnArgs),

  /// Show one page in detail: layout, buffers, chunks, and repetition index.
  Show(ShowArgs),
}
