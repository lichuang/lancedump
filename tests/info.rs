// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    .args(["info", "--format", "json"])
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

fn run_raw(args: &[&str]) -> Output {
  ldp().args(args).output().expect("failed to run ldp")
}

#[test]
fn info_json_v2_1() {
  let report = run_json("v2_1.lance");
  assert_eq!(report["format"]["name"], "v2.1");
  assert_eq!(report["format"]["major"], 2);
  assert_eq!(report["format"]["minor"], 1);
  assert_eq!(report["num_rows"], 256);
  assert_eq!(report["num_columns"], 1);
  assert_eq!(report["num_global_buffers"], 1);
  assert_eq!(report["schema"]["fields"], 1);
  assert_eq!(report["cmo"][0]["pages"], 1);
  assert_eq!(report["cmo"][0]["rows"], 256);
  assert_eq!(report["cmo"][0]["bytes"], 16_424);
  assert_eq!(report["regions"].as_array().unwrap().len(), 5);
  assert_eq!(report["cmo"].as_array().unwrap().len(), 1);
  assert_eq!(report["cmo"][0]["column"], 0);
  assert!(
    report["global_buffers"][0]["description"]
      .as_str()
      .unwrap()
      .contains("schema (FileDescriptor")
  );
  assert_eq!(report["warnings"].as_array().unwrap().len(), 0);
}

#[test]
fn info_json_v2_2() {
  let report = run_json("v2_2.lance");
  assert_eq!(report["format"]["name"], "v2.2");
  assert_eq!(report["num_rows"], 66_049);
  assert_eq!(report["num_columns"], 2);
  assert_eq!(report["cmo"].as_array().unwrap().len(), 2);

  let cmo = report["cmo"].as_array().unwrap();
  assert_eq!(cmo[0]["pages"], 1);
  assert_eq!(cmo[0]["bytes"], 30);
  assert_eq!(cmo[0]["buffers"], 1);
  assert_eq!(cmo[1]["pages"], 1);
  assert!(cmo[1]["bytes"].as_u64().unwrap() > 50_000);
}

#[test]
fn info_json_v2_0_uses_footer_0_3() {
  let report = run_json("v2_0.lance");
  // 0.3 is v2.0 in Lance's footer numbering, not a legacy v1 file.
  assert_eq!(report["format"]["major"], 0);
  assert_eq!(report["format"]["minor"], 3);
  assert_eq!(report["format"]["name"], "v2.0");
  assert!(report["schema"].is_object());
}

#[test]
fn info_legacy_v1_fails() {
  let output = run_raw(&["info", fixture("v1.lance").to_str().unwrap()]);
  assert!(!output.status.success());
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(stderr.contains("unsupported"), "stderr: {stderr}");
  assert!(stderr.contains("v1"), "stderr: {stderr}");
}

#[test]
fn info_text_has_sections() {
  let output = run_raw(&["info", fixture("v2_2.lance").to_str().unwrap()]);
  assert!(output.status.success());
  let text = String::from_utf8(output.stdout).unwrap();
  assert!(text.starts_with("Summary"));
  assert!(text.contains("Format"));
  assert!(text.contains("Footer\n"));
  assert!(text.contains("column_meta_start"));
  assert!(text.contains("Regions"));
  assert!(text.contains("Column Metadata Offset Table"));
  assert!(text.contains("Global buffers"));
  assert!(text.contains("Global Buffer Offset Table"));
  assert!(text.contains("v2.2 (2.2)"));
}

#[test]
fn info_forced_color_emits_ansi() {
  let output = ldp()
    .env_remove("NO_COLOR")
    .env("CLICOLOR_FORCE", "1")
    .args(["info", fixture("v2_1.lance").to_str().unwrap()])
    .output()
    .unwrap();
  assert!(output.status.success());
  let text = String::from_utf8(output.stdout).unwrap();
  assert!(text.contains("\x1b[1;36m"));
  assert!(text.contains("Summary"));
}

#[test]
fn info_truncated_file_fails() {
  let path = temp_path("truncated.lance");
  let bytes = fs::read(fixture("v2_2.lance")).unwrap();
  fs::write(&path, &bytes[..20]).unwrap();

  let output = run_raw(&["info", path.to_str().unwrap()]);
  fs::remove_file(&path).ok();

  assert!(!output.status.success());
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(
    stderr.contains("too small") || stderr.contains("footer") || stderr.contains("magic"),
    "unexpected stderr: {stderr}"
  );
}

#[test]
fn info_non_lance_file_fails() {
  let path = temp_path("not-a-lance-file");
  fs::write(&path, [0u8; 40]).unwrap();

  let output = run_raw(&["info", path.to_str().unwrap()]);
  fs::remove_file(&path).ok();

  assert!(!output.status.success());
  assert!(String::from_utf8_lossy(&output.stderr).contains("magic"));
}

fn temp_path(name: &str) -> PathBuf {
  std::env::temp_dir().join(format!("ldp-test-{}-{name}", std::process::id()))
}
