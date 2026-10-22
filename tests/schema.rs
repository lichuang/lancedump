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

fn run_json(name: &str) -> Value {
  let output = ldp()
    .args(["schema", "--format", "json"])
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
fn schema_json_v2_2() {
  let report = run_json("v2_2.lance");
  assert_eq!(report["fields"], 3);

  let tree = report["tree"].as_array().unwrap();
  assert_eq!(tree.len(), 2);
  assert_eq!(tree[0]["name"], "mime");
  assert_eq!(tree[0]["logical_type"], "string");
  assert_eq!(tree[0]["nullable"], true);

  assert_eq!(tree[1]["name"], "captions");
  assert_eq!(tree[1]["logical_type"], "list");
  let children = tree[1]["children"].as_array().unwrap();
  assert_eq!(children.len(), 1);
  assert_eq!(children[0]["name"], "item");
  assert_eq!(children[0]["path"], "captions.item");
  assert_eq!(children[0]["logical_type"], "uint32");
}

#[test]
fn schema_text_v2_2() {
  let output = ldp()
    .args(["schema"])
    .arg(fixture("v2_2.lance"))
    .output()
    .unwrap();
  assert!(output.status.success());
  let text = String::from_utf8(output.stdout).unwrap();
  assert!(text.contains("Fields: 3"));
  assert!(text.contains("id  field"));
  assert!(text.contains("mime"));
  assert!(text.contains("captions"));
  assert!(text.contains("└─ item"));
  assert!(text.contains("captions.item"));
}

#[test]
fn schema_legacy_v1_fails_clearly() {
  let output = ldp()
    .args(["schema"])
    .arg(fixture("v1.lance"))
    .output()
    .unwrap();
  assert!(!output.status.success());
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(stderr.contains("unsupported"), "stderr: {stderr}");
  assert!(stderr.contains("v1"), "stderr: {stderr}");
}
