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

pub mod encodings_v2_0 {
  #![allow(dead_code, clippy::all)]
  include!(concat!(env!("OUT_DIR"), "/lance.encodings.rs"));
}

pub mod encodings_v2_1 {
  #![allow(dead_code, clippy::all)]
  include!(concat!(env!("OUT_DIR"), "/lance.encodings21.rs"));
}

pub use encodings_v2_0 as pb20;
pub use encodings_v2_1 as pb21;
pub use file as pb;
pub use file_v2 as pb2;

/// Return the fully-qualified protobuf message name from an `Any` type URL.
///
/// Lance writes type URLs such as `/lance.encodings21.PageLayout`, but the
/// canonical form is `type.googleapis.com/lance.encodings21.PageLayout`; in
/// both cases the name is the last path segment.
pub fn any_message_name(type_url: &str) -> &str {
  type_url.rsplit('/').next().unwrap_or(type_url)
}

/// Return the short message name (`PageLayout`, `ArrayEncoding`, ...).
pub fn any_type_name(type_url: &str) -> &str {
  any_message_name(type_url)
    .rsplit('.')
    .next()
    .unwrap_or(type_url)
}
