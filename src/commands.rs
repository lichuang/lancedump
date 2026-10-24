// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use anyhow::Result;

use crate::cli::{Cli, Command};
use crate::format::container::FileMeta;
use crate::report::emit;
use crate::report::info::InfoReport;
use crate::report::pages::PagesReport;
use crate::report::schema::SchemaReport;
use crate::report::show::{ShowOptions, ShowReport};
use crate::source::FileSource;

pub fn run(cli: Cli) -> Result<()> {
  match cli.command {
    Command::Info(args) => {
      let meta = FileMeta::open(&args.path)?;
      emit(&InfoReport::from_meta(&meta), cli.format)
    }
    Command::Schema(args) => {
      let meta = FileMeta::open(&args.path)?;
      emit(&SchemaReport::from_schema(meta.schema()?), cli.format)
    }
    Command::Pages(args) => {
      let meta = FileMeta::open(&args.path)?;
      emit(&PagesReport::from_meta(&meta, args.column)?, cli.format)
    }
    Command::Show(args) => {
      let mut source = FileSource::open(&args.path)?;
      let meta = FileMeta::open_from(&mut source)?;
      let options = ShowOptions {
        hex: args.hex,
        hex_limit: args.hex_limit,
        raw: args.raw,
        detail: args.detail,
      };
      let report = ShowReport::from_meta(&meta, args.page, &mut source, options)?;
      emit(&report, cli.format)
    }
  }
}
