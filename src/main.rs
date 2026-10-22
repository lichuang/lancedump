// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

mod cli;
mod commands;
mod format;
mod proto;
mod report;
mod source;
mod view;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;

fn main() -> ExitCode {
  let cli = Cli::parse();
  match commands::run(cli) {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("error: {error:#}");
      ExitCode::FAILURE
    }
  }
}
