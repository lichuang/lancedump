// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Generated protobuf types for the Lance file format.
//!
//! Only the container layer is compiled for now: `file.proto` (schema and
//! `FileDescriptor`) and `file2.proto` (column metadata and page buffers).

pub mod file {
  #![allow(dead_code, clippy::all)]
  include!(concat!(env!("OUT_DIR"), "/lance.file.rs"));
}

pub mod file_v2 {
  #![allow(dead_code, clippy::all)]
  include!(concat!(env!("OUT_DIR"), "/lance.file.v2.rs"));
}

pub use file as pb;
pub use file_v2 as pb2;
