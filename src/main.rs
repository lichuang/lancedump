// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

mod cli;
mod format;
mod proto;
mod report;
mod source;
mod view;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::format::container::FileMeta;
use crate::report::emit;
use crate::report::info::InfoReport;
use crate::report::schema::SchemaReport;

fn main() -> ExitCode {
  let cli = Cli::parse();
  match run(cli) {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("error: {error:#}");
      ExitCode::FAILURE
    }
  }
}

fn run(cli: Cli) -> Result<()> {
  match cli.command {
    Command::Info(args) => {
      let meta = FileMeta::open(&args.path)?;
      let report = InfoReport::from_meta(&meta);
      emit(&report, cli.format)
    }
    Command::Schema(args) => {
      let meta = FileMeta::open(&args.path)?;
      let report = SchemaReport::from_schema(meta.schema()?);
      emit(&report, cli.format)
    }
  }
}
