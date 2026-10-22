// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn ldp() -> Command {
  Command::new(env!("CARGO_BIN_EXE_ldp"))
}

fn fixture(name: &str) -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests/fixtures")
    .join(name)
}

fn run_json(name: &str, column: Option<u32>) -> Value {
  let mut command = ldp();
  command.args(["pages", "--format", "json"]);
  if let Some(column) = column {
    command.args(["--column", &column.to_string()]);
  }
  let output = command
    .arg(fixture(name))
    .output()
    .expect("failed to run ldp");
  assert!(
    output.status.success(),
    "ldp failed: {}",
    String::from_utf8_lossy(&output.stderr)
  );
  serde_json::from_slice(&output.stdout).expect("invalid JSON output")
}

#[test]
fn pages_json_v2_1() {
  let report = run_json("v2_1.lance", None);
  let pages = report["pages"].as_array().unwrap();
  assert_eq!(pages.len(), 1);
  assert_eq!(pages[0]["column"], 0);
  assert_eq!(pages[0]["page"], 0);
  assert_eq!(pages[0]["rows"], 256);
  assert_eq!(pages[0]["start"], 0);
  assert_eq!(pages[0]["end"], 16_480);
  assert_eq!(pages[0]["bytes"], 16_424);
  assert_eq!(pages[0]["layout"], "mini-block");
  assert!(pages[0]["detail"].as_str().unwrap().contains("u16"));
  assert!(
    pages[0]["type_url"]
      .as_str()
      .unwrap()
      .contains("PageLayout")
  );
}

#[test]
fn pages_json_v2_2() {
  let report = run_json("v2_2.lance", None);
  let pages = report["pages"].as_array().unwrap();
  assert_eq!(pages.len(), 2);

  assert_eq!(pages[0]["column"], 0);
  assert_eq!(pages[0]["start"], 0);
  assert_eq!(pages[0]["end"], 30);
  assert_eq!(pages[0]["layout"], "constant");
  assert_eq!(pages[0]["detail"], "all-null");

  assert_eq!(pages[0]["global_index"], 0);
  assert_eq!(pages[1]["global_index"], 1);
  assert!(pages[0]["start"].as_u64() <= pages[1]["start"].as_u64());
  assert_eq!(pages[1]["column"], 1);
  assert_eq!(pages[1]["layout"], "mini-block");
  assert!(pages[1]["detail"].as_str().unwrap().contains("u32"));
  assert_eq!(pages[1]["buffers"], 3);
}

#[test]
fn pages_filters_by_column() {
  let report = run_json("v2_2.lance", Some(1));
  let pages = report["pages"].as_array().unwrap();
  assert_eq!(pages.len(), 1);
  assert_eq!(pages[0]["column"], 1);
  assert_eq!(pages[0]["global_index"], 1);
}

#[test]
fn pages_json_v2_0_uses_legacy_layout() {
  let report = run_json("v2_0.lance", None);
  let pages = report["pages"].as_array().unwrap();
  assert_eq!(pages.len(), 1);
  assert_eq!(pages[0]["layout"], "legacy-v2.0");
  assert_eq!(pages[0]["detail"], "nullable");
}

#[test]
fn pages_text_v2_1() {
  let output = ldp()
    .args(["pages", fixture("v2_1.lance").to_str().unwrap()])
    .output()
    .unwrap();
  assert!(output.status.success());
  let text = String::from_utf8(output.stdout).unwrap();
  assert!(text.contains("column"));
  assert!(text.contains("page (in column)"));
  assert!(text.contains("#  column"));
  assert!(text.contains("mini-block (u16 chunk meta"));
}

#[test]
fn pages_colorizes_layouts() {
  let output = ldp()
    .env_remove("NO_COLOR")
    .env("CLICOLOR_FORCE", "1")
    .args(["pages", fixture("v2_2.lance").to_str().unwrap()])
    .output()
    .unwrap();
  assert!(output.status.success());
  let text = String::from_utf8(output.stdout).unwrap();
  // constant -> blue, mini-block -> green
  assert!(text.contains("\x1b[34mconstant"));
  assert!(text.contains("\x1b[32mmini-block"));
}

#[test]
fn pages_missing_column_fails() {
  let output = ldp()
    .args([
      "pages",
      "--column",
      "9",
      fixture("v2_1.lance").to_str().unwrap(),
    ])
    .output()
    .unwrap();
  assert!(!output.status.success());
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(stderr.contains("does not exist"), "stderr: {stderr}");
}
