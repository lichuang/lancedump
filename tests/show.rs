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

fn run_json(name: &str, page: u32, extra: &[&str]) -> Value {
  let mut command = ldp();
  command.args(["show", "--format", "json"]);
  command.args(["--page", &page.to_string()]);
  command.args(extra);
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
fn show_json_v2_1_miniblock() {
  let report = run_json("v2_1.lance", 0, &[]);
  assert_eq!(report["index"], 0);
  assert_eq!(report["column"], 0);
  assert_eq!(report["page"], 0);
  assert_eq!(report["start"], 0);
  assert_eq!(report["end"], 16_480);
  assert_eq!(report["rows"], 256);
  assert_eq!(report["layout"]["kind"], "mini-block");

  let buffers = report["buffers"].as_array().unwrap();
  assert_eq!(buffers.len(), 2);
  assert_eq!(buffers[0]["role"], "chunk metadata");
  assert_eq!(buffers[1]["role"], "chunk data");
  assert_eq!(buffers[1]["start"], 64);

  let chunks = &report["internals"]["chunks"];
  assert_eq!(chunks["num_chunks"], 4);
  assert_eq!(chunks["num_items"], 256);
  assert_eq!(chunks["total_values"], 256);
  assert_eq!(chunks["chunks"][0]["offset"], 0);
  assert_eq!(chunks["chunks"][0]["start"], 64);
  assert_eq!(chunks["chunks"][0]["metadata_word"], 8_198);
  assert_eq!(chunks["chunks"][0]["bytes"], 4_104);
  assert_eq!(chunks["chunks"][0]["values"], 64);
  assert_eq!(chunks["chunks"][3]["values"], 64);
  assert_eq!(report["internals"]["kind"], "mini-block");
  assert_eq!(report["warnings"].as_array().unwrap().len(), 0);
}

#[test]
fn show_json_v2_2_miniblock_with_repdef() {
  let report = run_json("v2_2.lance", 1, &[]);
  assert_eq!(report["index"], 1);
  assert_eq!(report["column"], 1);
  let buffers = report["buffers"].as_array().unwrap();
  assert_eq!(buffers.len(), 3);
  assert_eq!(buffers[2]["role"], "repetition index");

  let chunks = &report["internals"]["chunks"];
  assert_eq!(chunks["num_chunks"], 17);
  assert_eq!(chunks["num_items"], 16_416);
  assert_eq!(chunks["total_values"], 16_416);

  let last = &chunks["chunks"][16];
  assert_eq!(last["values"], 32);
  assert_eq!(last["num_levels"], 32);
  assert!(last["rep_buffer"][1].as_u64().unwrap() > 0);
  assert!(last["def_buffer"][1].as_u64().unwrap() > 0);
  assert_eq!(last["start"], 34_624);
  assert_eq!(last["rep_index"][0], 65_537);
  assert_eq!(last["rep_index"][1], 0);
}

#[test]
fn show_json_constant_page() {
  let report = run_json("v2_2.lance", 0, &[]);
  assert_eq!(report["layout"]["kind"], "constant");
  assert!(report["internals"].is_null());

  let fields = report["layout"]["fields"].as_array().unwrap();
  assert!(fields.iter().any(|field| field["name"] == "inline_value"
    && field["value"].as_str().unwrap().contains("all-null")));
}

#[test]
fn show_json_legacy_page() {
  let report = run_json("v2_0.lance", 0, &[]);
  assert_eq!(report["layout"]["kind"], "legacy-v2.0");
  assert!(report["internals"].is_null());
  assert_eq!(report["buffers"][0]["role"], "legacy array buffer");
}

#[test]
fn show_hex_and_raw() {
  let report = run_json("v2_1.lance", 0, &["--hex", "--hex-limit", "16", "--raw"]);
  let hexdump = report["hexdump"].as_array().unwrap();
  assert!(hexdump.len() >= 2);
  assert!(hexdump[0].as_str().unwrap().contains("buffer 0 @ 0"));
  assert!(report["raw"].as_str().unwrap().contains("PageLayout"));
}

#[test]
fn show_detail_v2_2_levels_and_values() {
  let report = run_json("v2_2.lance", 1, &["--detail"]);
  let chunk = &report["internals"]["chunks"]["chunks"][0];

  let rep = chunk["rep_levels"].as_array().unwrap();
  assert_eq!(rep.len(), 1024);
  assert_eq!(rep[0], 1);
  assert_eq!(rep[1], 0);

  let def = chunk["def_levels"].as_array().unwrap();
  assert_eq!(def.len(), 1024);
  assert_eq!(def[0], 0);

  let values = &chunk["decoded_values"];
  assert_eq!(values["kind"], "fixed");
  assert_eq!(values["bits_per_value"], 32);
  assert_eq!(values["items_per_value"], 1);
  assert_eq!(values["values"].as_array().unwrap().len(), 1024);
  assert_eq!(values["values"][0], 0);
  assert_eq!(values["values"][2], 2);

  assert_eq!(chunk["value_buffer_sizes"], serde_json::json!([1_284]));

  // Detail fields are absent without --detail.
  let plain = run_json("v2_2.lance", 1, &[]);
  assert!(
    plain["internals"]["chunks"]["chunks"][0]
      .get("rep_levels")
      .is_none()
  );
}

#[test]
fn show_detail_v2_1_fixed_size_list() {
  let report = run_json("v2_1.lance", 0, &["--detail"]);
  let chunk = &report["internals"]["chunks"]["chunks"][0];
  assert!(chunk.get("rep_levels").is_none());
  assert!(chunk.get("def_levels").is_none());

  let values = &chunk["decoded_values"];
  assert_eq!(values["kind"], "fixed");
  assert_eq!(values["bits_per_value"], 32);
  assert_eq!(values["items_per_value"], 16);
  assert_eq!(values["values"].as_array().unwrap().len(), 1024);
}

#[test]
fn show_detail_variable_values() {
  let report = run_json("variable_strings.lance", 0, &["--detail"]);
  let chunk = &report["internals"]["chunks"]["chunks"][0];
  let values = &chunk["decoded_values"];
  assert_eq!(values["kind"], "variable");
  assert_eq!(values["bits_per_offset"], 32);
  assert_eq!(values["values"], serde_json::json!(["a", "b", "c", "d"]));
  assert_eq!(values["sizes"], serde_json::json!([1, 1, 1, 1]));

  let output = ldp()
    .args([
      "show",
      "-d",
      "--page",
      "0",
      fixture("variable_strings.lance").to_str().unwrap(),
    ])
    .output()
    .unwrap();
  assert!(output.status.success());
  let text = String::from_utf8(output.stdout).unwrap();
  assert!(text.contains("values (4 values, variable, 32-bit offsets)"));
  assert!(text.contains("[0] (1 bytes) a"));
}

#[test]
fn show_text_v2_1() {
  let output = ldp()
    .args([
      "show",
      "--page",
      "0",
      fixture("v2_1.lance").to_str().unwrap(),
    ])
    .output()
    .unwrap();
  assert!(output.status.success());
  let text = String::from_utf8(output.stdout).unwrap();
  assert!(text.starts_with("Summary"));
  assert!(text.contains("Page index in column"));
  assert!(text.contains("Layout: mini-block"));
  assert!(text.contains("Page buffers"));
  assert!(text.contains("Mini-block chunks (4 chunks, 256 items"));
}

#[test]
fn show_missing_page_fails() {
  let output = ldp()
    .args([
      "show",
      "--page",
      "9",
      fixture("v2_1.lance").to_str().unwrap(),
    ])
    .output()
    .unwrap();
  assert!(!output.status.success());
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(stderr.contains("page 9 does not exist"), "stderr: {stderr}");
}
