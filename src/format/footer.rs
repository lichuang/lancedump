// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Lance file footer parsing.
//!
//! There are two footer layouts:
//!
//! ```text
//! v1 (major 0, minor 0..=2), 16 bytes:
//!   i64 metadata_position
//!   i16 major, i16 minor
//!   "LANC"
//!
//! v2 (major 0/minor 3, or major 2/minor 0..=3), 40 bytes:
//!   u64 column_meta_start
//!   u64 column_meta_offsets_start
//!   u64 global_buff_offsets_start
//!   u32 num_global_buffers
//!   u32 num_columns
//!   u16 major, u16 minor
//!   "LANC"
//! ```
//!
//! The version can be read from the last 8 bytes before the magic in both
//! layouts, so a single tail read is enough to choose the parser.

use std::fmt;

use anyhow::{Result, bail};

pub const MAGIC: &[u8; 4] = b"LANC";
/// Size of the footer of a v2 file.
pub const FOOTER_LEN_V2: u64 = 40;
/// Size of the footer of a legacy v1 file.
pub const FOOTER_LEN_V1: u64 = 16;

/// Exact file format version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
  /// Legacy v1 (footer major 0, minor 0..=2), 16-byte footer.
  Legacy {
    minor: u16,
  },
  V2_0,
  V2_1,
  V2_2,
  V2_3,
  Unknown {
    major: u16,
    minor: u16,
  },
}

impl Version {
  pub fn from_numbers(major: u16, minor: u16) -> Self {
    match (major, minor) {
      // v1 wrote 0.1 and 0.2; both use the 16-byte footer.
      (0, 0..=2) => Self::Legacy { minor },
      // 0.3 and 2.0 are both v2.0. Embedded/mini-lance writers use 2.0.
      (0, 3) | (2, 0) => Self::V2_0,
      (2, 1) => Self::V2_1,
      (2, 2) => Self::V2_2,
      (2, 3) => Self::V2_3,
      _ => Self::Unknown { major, minor },
    }
  }

  pub fn is_v2(self) -> bool {
    matches!(self, Self::V2_0 | Self::V2_1 | Self::V2_2 | Self::V2_3)
  }
}

impl fmt::Display for Version {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Legacy { .. } => write!(f, "v1"),
      Self::V2_0 => write!(f, "v2.0"),
      Self::V2_1 => write!(f, "v2.1"),
      Self::V2_2 => write!(f, "v2.2"),
      Self::V2_3 => write!(f, "v2.3"),
      Self::Unknown { major, minor } => write!(f, "unknown ({major}.{minor})"),
    }
  }
}

/// Footer of a v2 file.
#[derive(Debug, Clone, Copy)]
pub struct V2Footer {
  pub column_meta_start: u64,
  pub column_meta_offsets_start: u64,
  pub global_buff_offsets_start: u64,
  pub num_global_buffers: u32,
  pub num_columns: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum FooterLayout {
  V2(V2Footer),
  /// The embedded metadata position of a v1 file.
  Legacy {
    metadata_position: u64,
  },
  Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct Footer {
  pub major_version: u16,
  pub minor_version: u16,
  pub layout: FooterLayout,
}

impl Footer {
  /// Parse the tail of a file.
  ///
  /// `tail` must be the last `min(40, file_size)` bytes, so that the 16-byte
  /// v1 footer and the 40-byte v2 footer can both be addressed.
  pub fn parse(tail: &[u8]) -> Result<Self> {
    let len = tail.len();
    if len < FOOTER_LEN_V1 as usize {
      bail!("file is too small for a Lance footer: {len} bytes (need at least {FOOTER_LEN_V1})");
    }
    if &tail[len - 4..] != MAGIC {
      bail!(
        "not a Lance file: expected magic {:?}, found {:?}",
        std::str::from_utf8(MAGIC).unwrap(),
        &tail[len - 4..]
      );
    }

    let major_version = u16_at(tail, len - 8)?;
    let minor_version = u16_at(tail, len - 6)?;
    let version = Version::from_numbers(major_version, minor_version);

    let layout = match version {
      Version::Legacy { .. } => FooterLayout::Legacy {
        metadata_position: u64_at(tail, len - 16)?,
      },
      Version::V2_0 | Version::V2_1 | Version::V2_2 | Version::V2_3 => {
        if len < FOOTER_LEN_V2 as usize {
          bail!("file is too small for a v2 Lance footer: {len} bytes (need {FOOTER_LEN_V2})");
        }
        FooterLayout::V2(V2Footer {
          column_meta_start: u64_at(tail, len - 40)?,
          column_meta_offsets_start: u64_at(tail, len - 32)?,
          global_buff_offsets_start: u64_at(tail, len - 24)?,
          num_global_buffers: u32_at(tail, len - 16)?,
          num_columns: u32_at(tail, len - 12)?,
        })
      }
      Version::Unknown { .. } => FooterLayout::Unknown,
    };

    Ok(Self {
      major_version,
      minor_version,
      layout,
    })
  }

  pub fn version(&self) -> Version {
    Version::from_numbers(self.major_version, self.minor_version)
  }

  pub fn v2(&self) -> Option<&V2Footer> {
    match &self.layout {
      FooterLayout::V2(footer) => Some(footer),
      _ => None,
    }
  }

  /// The five regions of a v2 file, in file order.
  pub fn regions(&self, file_size: u64) -> Option<Vec<Region>> {
    let footer = self.v2()?;
    Some(vec![
      Region {
        name: "data",
        start: 0,
        end: footer.column_meta_start,
      },
      Region {
        name: "column metadata",
        start: footer.column_meta_start,
        end: footer.column_meta_offsets_start,
      },
      Region {
        name: "column metadata offset table",
        start: footer.column_meta_offsets_start,
        end: footer.global_buff_offsets_start,
      },
      Region {
        name: "global buffer offset table",
        start: footer.global_buff_offsets_start,
        end: file_size.saturating_sub(FOOTER_LEN_V2),
      },
      Region {
        name: "footer",
        start: file_size.saturating_sub(FOOTER_LEN_V2),
        end: file_size,
      },
    ])
  }
}

#[derive(Debug, Clone, Copy)]
pub struct Region {
  pub name: &'static str,
  pub start: u64,
  pub end: u64,
}

impl Region {
  pub fn size(&self) -> u64 {
    self.end.saturating_sub(self.start)
  }
}

fn u64_at(buf: &[u8], at: usize) -> Result<u64> {
  let slice = buf
    .get(at..at + 8)
    .ok_or_else(|| anyhow::anyhow!("footer is too short at offset {at}"))?;
  Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

fn u32_at(buf: &[u8], at: usize) -> Result<u32> {
  let slice = buf
    .get(at..at + 4)
    .ok_or_else(|| anyhow::anyhow!("footer is too short at offset {at}"))?;
  Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn u16_at(buf: &[u8], at: usize) -> Result<u16> {
  let slice = buf
    .get(at..at + 2)
    .ok_or_else(|| anyhow::anyhow!("footer is too short at offset {at}"))?;
  Ok(u16::from_le_bytes(slice.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn v2_footer(
    column_meta_start: u64,
    cmo_start: u64,
    gbo_start: u64,
    num_global: u32,
    num_columns: u32,
    major: u16,
    minor: u16,
  ) -> Vec<u8> {
    let mut out = Vec::with_capacity(40);
    out.extend_from_slice(&column_meta_start.to_le_bytes());
    out.extend_from_slice(&cmo_start.to_le_bytes());
    out.extend_from_slice(&gbo_start.to_le_bytes());
    out.extend_from_slice(&num_global.to_le_bytes());
    out.extend_from_slice(&num_columns.to_le_bytes());
    out.extend_from_slice(&major.to_le_bytes());
    out.extend_from_slice(&minor.to_le_bytes());
    out.extend_from_slice(MAGIC);
    out
  }

  fn v1_footer(metadata_position: u64, minor: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&metadata_position.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&minor.to_le_bytes());
    out.extend_from_slice(MAGIC);
    out
  }

  #[test]
  fn parses_v2_footer() {
    let bytes = v2_footer(100, 200, 300, 1, 2, 2, 2);
    let footer = Footer::parse(&bytes).unwrap();
    assert_eq!(footer.version(), Version::V2_2);
    let v2 = footer.v2().unwrap();
    assert_eq!(v2.column_meta_start, 100);
    assert_eq!(v2.column_meta_offsets_start, 200);
    assert_eq!(v2.global_buff_offsets_start, 300);
    assert_eq!(v2.num_global_buffers, 1);
    assert_eq!(v2.num_columns, 2);
  }

  #[test]
  fn parses_legacy_footer() {
    let bytes = v1_footer(321, 1);
    let footer = Footer::parse(&bytes).unwrap();
    assert_eq!(footer.version(), Version::Legacy { minor: 1 });
    assert!(matches!(
      footer.layout,
      FooterLayout::Legacy {
        metadata_position: 321
      }
    ));
    assert!(footer.v2().is_none());
  }

  #[test]
  fn maps_versions() {
    assert_eq!(Version::from_numbers(0, 0), Version::Legacy { minor: 0 });
    assert_eq!(Version::from_numbers(0, 2), Version::Legacy { minor: 2 });
    assert_eq!(Version::from_numbers(0, 3), Version::V2_0);
    assert_eq!(Version::from_numbers(2, 0), Version::V2_0);
    assert_eq!(Version::from_numbers(2, 3), Version::V2_3);
    assert!(matches!(
      Version::from_numbers(1, 9),
      Version::Unknown { major: 1, minor: 9 }
    ));
  }

  #[test]
  fn rejects_bad_magic() {
    let mut bytes = v2_footer(0, 0, 0, 0, 0, 2, 2);
    *bytes.last_mut().unwrap() = b'X';
    let error = Footer::parse(&bytes).unwrap_err().to_string();
    assert!(error.contains("magic"));
  }

  #[test]
  fn rejects_short_input() {
    let error = Footer::parse(&[0u8; 8]).unwrap_err().to_string();
    assert!(error.contains("too small"));
  }

  #[test]
  fn rejects_short_v2_footer() {
    let bytes = v2_footer(0, 0, 0, 0, 0, 2, 2);
    let error = Footer::parse(&bytes[16..]).unwrap_err().to_string();
    assert!(error.contains("v2 Lance footer"));
  }
}
