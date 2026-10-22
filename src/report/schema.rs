// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

use serde::Serialize;

use crate::proto::pb;
use crate::report::{Report, metadata_map};
use crate::view::{Style, Table};

#[derive(Debug, Serialize)]
pub struct SchemaNode {
  pub id: i32,
  /// Fully qualified path, e.g. `st.mini.item`.
  pub path: String,
  pub name: String,
  pub logical_type: String,
  pub nullable: bool,
  pub metadata: BTreeMap<String, String>,
  pub children: Vec<SchemaNode>,
}

#[derive(Debug, Serialize)]
pub struct SchemaReport {
  pub fields: usize,
  pub metadata: BTreeMap<String, String>,
  pub tree: Vec<SchemaNode>,
}

impl SchemaReport {
  pub fn from_schema(schema: &pb::Schema) -> Self {
    let by_id: HashMap<i32, usize> = schema
      .fields
      .iter()
      .enumerate()
      .map(|(index, field)| (field.id, index))
      .collect();

    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (index, field) in schema.fields.iter().enumerate() {
      if field.parent_id < 0 {
        roots.push(index);
        continue;
      }
      match by_id.get(&field.parent_id) {
        Some(parent) => children.entry(*parent).or_default().push(index),
        // A missing parent means a malformed tree; treat the field as a
        // root instead of dropping it.
        None => roots.push(index),
      }
    }

    let mut visited = HashSet::new();
    let tree = roots
      .into_iter()
      .filter_map(|index| build_node(index, "", schema, &children, &mut visited, 0))
      .collect();

    Self {
      fields: schema.fields.len(),
      metadata: metadata_map(&schema.metadata),
      tree,
    }
  }
}

impl Report for SchemaReport {
  fn render_text(&self, style: Style) -> String {
    let mut out = String::new();
    let _ = writeln!(
      out,
      "{}",
      style.heading(&format!("Fields: {}", self.fields))
    );

    if !self.tree.is_empty() {
      out.push('\n');
      let mut table = Table::new(&["id", "field", "type", "nullable", "path"]).right_align(&[0]);
      for node in &self.tree {
        push_node(&mut table, node, "", "");
      }
      out.push_str(&table.render());
    }

    let mut field_metadata = Vec::new();
    collect_field_metadata(&self.tree, &mut field_metadata);
    if !field_metadata.is_empty() {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Field metadata"));
      for (path, metadata) in &field_metadata {
        let _ = writeln!(out, "  {path}");
        for (key, value) in metadata.iter() {
          let _ = writeln!(out, "    {key} = {value}");
        }
      }
    }

    if !self.metadata.is_empty() {
      out.push('\n');
      let _ = writeln!(out, "{}", style.heading("Schema metadata"));
      for (key, value) in &self.metadata {
        let _ = writeln!(out, "  {key} = {value}");
      }
    }

    out
  }
}

fn push_node(table: &mut Table, node: &SchemaNode, prefix: &str, connector: &str) {
  table.push(vec![
    node.id.to_string(),
    format!("{prefix}{connector}{}", node.name),
    node.logical_type.clone(),
    if node.nullable { "yes" } else { "no" }.to_string(),
    node.path.clone(),
  ]);

  let child_prefix = match connector {
    "├─ " => format!("{prefix}│  "),
    "└─ " => format!("{prefix}   "),
    _ => prefix.to_string(),
  };
  for (index, child) in node.children.iter().enumerate() {
    let last = index + 1 == node.children.len();
    push_node(
      table,
      child,
      &child_prefix,
      if last { "└─ " } else { "├─ " },
    );
  }
}

fn collect_field_metadata<'a>(
  nodes: &'a [SchemaNode],
  out: &mut Vec<(&'a str, &'a BTreeMap<String, String>)>,
) {
  for node in nodes {
    if !node.metadata.is_empty() {
      out.push((&node.path, &node.metadata));
    }
    collect_field_metadata(&node.children, out);
  }
}

fn build_node(
  index: usize,
  parent_path: &str,
  schema: &pb::Schema,
  children: &HashMap<usize, Vec<usize>>,
  visited: &mut HashSet<usize>,
  depth: usize,
) -> Option<SchemaNode> {
  // Guard against malformed parent links that form a cycle.
  if depth > 64 || !visited.insert(index) {
    return None;
  }
  let field = &schema.fields[index];
  let path = if parent_path.is_empty() {
    field.name.clone()
  } else {
    format!("{parent_path}.{}", field.name)
  };
  let child_nodes = children
    .get(&index)
    .map(|indices| {
      indices
        .iter()
        .filter_map(|child| build_node(*child, &path, schema, children, visited, depth + 1))
        .collect()
    })
    .unwrap_or_default();

  Some(SchemaNode {
    id: field.id,
    path,
    name: field.name.clone(),
    logical_type: field.logical_type.clone(),
    nullable: field.nullable,
    metadata: metadata_map(&field.metadata),
    children: child_nodes,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn field(id: i32, parent_id: i32, name: &str, logical_type: &str) -> pb::Field {
    pb::Field {
      r#type: pb::field::Type::Leaf as i32,
      name: name.to_string(),
      id,
      parent_id,
      logical_type: logical_type.to_string(),
      nullable: true,
      ..Default::default()
    }
  }

  #[test]
  fn builds_tree_with_paths() {
    let schema = pb::Schema {
      fields: vec![
        field(0, -1, "a", "string"),
        field(1, -1, "b", "list"),
        field(2, 1, "item", "uint32"),
      ],
      metadata: Default::default(),
    };
    let report = SchemaReport::from_schema(&schema);
    assert_eq!(report.fields, 3);
    assert_eq!(report.tree.len(), 2);
    assert_eq!(report.tree[1].path, "b");
    assert_eq!(report.tree[1].children.len(), 1);
    assert_eq!(report.tree[1].children[0].path, "b.item");
  }

  #[test]
  fn orphan_field_becomes_root() {
    let schema = pb::Schema {
      fields: vec![field(0, -1, "a", "string"), field(1, 99, "orphan", "int32")],
      metadata: Default::default(),
    };
    let report = SchemaReport::from_schema(&schema);
    assert_eq!(report.tree.len(), 2);
    assert_eq!(report.tree[1].path, "orphan");
  }

  #[test]
  fn cyclic_parent_links_produce_no_roots() {
    // 0 -> 1 -> 0: a malformed tree that must not recurse forever.
    let schema = pb::Schema {
      fields: vec![field(0, 1, "a", "struct"), field(1, 0, "b", "struct")],
      metadata: Default::default(),
    };
    let report = SchemaReport::from_schema(&schema);
    assert!(report.tree.is_empty());
  }
}
