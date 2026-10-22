// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::io::Result;

fn main() -> Result<()> {
  println!("cargo:rerun-if-changed=protos");
  println!("cargo:rerun-if-env-changed=PROTOC");

  #[cfg(feature = "vendored-protoc")]
  unsafe {
    std::env::set_var("PROTOC", protobuf_src::protoc());
  }

  let mut config = prost_build::Config::new();
  config.protoc_arg("--experimental_allow_proto3_optional");
  config.compile_protos(
    &["./protos/file.proto", "./protos/file2.proto"],
    &["./protos"],
  )?;

  Ok(())
}
