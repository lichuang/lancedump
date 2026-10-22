// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

pub mod info;
pub mod pages;
pub mod schema;
pub mod show;

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};

use anyhow::Result;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::view::Style;

pub trait Report: Serialize {
  fn render_text(&self, style: Style) -> String;
}

pub fn emit<T: Report>(report: &T, format: OutputFormat) -> Result<()> {
  let bytes = match format {
    OutputFormat::Text => report.render_text(style()).into_bytes(),
    OutputFormat::Json => {
      let mut bytes = serde_json::to_vec_pretty(report)?;
      bytes.push(b'\n');
      bytes
    }
  };
  write_stdout(&bytes)
}

/// Color is on for interactive terminals. `NO_COLOR` disables it;
/// `CLICOLOR_FORCE=1` forces it (useful when piping through `less -R`).
fn style() -> Style {
  use std::io::IsTerminal;

  let forced = std::env::var("CLICOLOR_FORCE")
    .map(|value| value != "0")
    .unwrap_or(false);
  let disabled = std::env::var_os("NO_COLOR").is_some();
  let terminal =
    std::io::stdout().is_terminal() && !matches!(std::env::var("TERM").as_deref(), Ok("dumb"));

  Style::new(!disabled && (forced || terminal))
}

/// Write to stdout, treating a closed pipe (`ldp ... | head`) as success.
fn write_stdout(bytes: &[u8]) -> Result<()> {
  let mut stdout = io::stdout().lock();
  match stdout.write_all(bytes).and_then(|()| stdout.flush()) {
    Ok(()) => Ok(()),
    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
    Err(error) => Err(error.into()),
  }
}

/// Schema metadata is `map<string, bytes>` in protobuf; render it lossily as
/// UTF-8 so it can be printed and serialized.
pub fn metadata_map(metadata: &HashMap<String, Vec<u8>>) -> BTreeMap<String, String> {
  metadata
    .iter()
    .map(|(key, value)| (key.clone(), String::from_utf8_lossy(value).into_owned()))
    .collect()
}
