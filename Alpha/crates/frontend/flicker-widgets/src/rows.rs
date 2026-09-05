//! **Data-driven rows** — a `list` authored with `rows_from: "<source>"` carries ONE
//! prototype child, and the scene expands it into one clone per row it publishes. A
//! `row` may repeat the same way (the shared file browser's breadcrumb: the path's
//! segments, one button each) — a repeater is a FLOW container, and the two differ only
//! in the axis they flow along.
//!
//! The authored tree stays STATIC (the scene never builds structure in Rust — the
//! five-line split, 491BD9BB). The expansion is a per-frame VALUE derived from data the
//! scene holds — a rig's 67 bones, a folder's candidate files, the socket roster —
//! exactly as `arrange()` lights slices from the selection. Read data, not code (Aaron
//! 2026-09-03): a skeleton with more bones is more rows, never more tree.
//!
//! Placeholders in the prototype: `{row}` is the row's ordinal (0-based) and `{id}` the
//! row's id — the data key a `radio` row's `value` carries and the list's shared `bind`
//! echoes when the row is picked. Both are substituted in the clone's `id`, `action`,
//! `visible_bind`, `enabled_bind` and every TEXT prop; the clone's `nav_ordinal` is the
//! prototype's plus the ordinal, so the d-pad walks the rows in order. The row's LABEL
//! never enters the tree as a literal: the clone's `label_bind` key (after substitution)
//! is published into the Model with the row's text, so the localisation gate's bind
//! exemption holds and the tree and the Model cannot drift apart.

use flicker_script::{UiNode, Value, ValueMap};

/// One row of a data-driven list: its stable id (the pick value) and its display text
/// (already final — a resolved `$token` or a name from the data).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub label: String,
}

impl Row {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// The `list` prop naming the row source a scene expands.
pub const ROWS_FROM: &str = "rows_from";

/// Expand every `rows_from` list in `tree` from the rows `source` yields for its name,
/// returning the tree the walker runs this frame and publishing each row's label into
/// `model`. A source the scene does not know is warned and expands to NO rows — visibly
/// empty, never a silent stale list (rule 4BB12A75).
pub fn instantiate_rows(
    tree: &UiNode,
    model: &mut ValueMap,
    source: &dyn Fn(&str) -> Option<Vec<Row>>,
) -> UiNode {
    let mut out = shallow(tree);
    let from = tree
        .props
        .get(ROWS_FROM)
        .and_then(|v| match v {
            Value::Text(s) if !s.is_empty() => Some(s.as_str()),
            _ => None,
        })
        // A repeater is a FLOW container: a `list` repeats down its scrolling column and
        // a `row` repeats across — the breadcrumb in the shared file browser is exactly
        // "the path's segments, one button each", the same data-driven rows turned
        // sideways. Any other kind naming `rows_from` is authoring noise and is ignored.
        .filter(|_| matches!(tree.component.as_str(), "list" | "row"));
    match from {
        Some(name) => {
            let Some(proto) = tree.children.first() else {
                tracing::warn!("rows: list `{}` has no prototype row to expand", tree.id);
                return out;
            };
            let Some(rows) = source(name) else {
                tracing::warn!(
                    "rows: list `{}` names row source `{name}` the scene does not publish",
                    tree.id
                );
                return out;
            };
            for (ordinal, row) in rows.iter().enumerate() {
                let clone = expand(proto, ordinal, &row.id);
                if let Some(Value::Text(key)) = clone.props.get("label_bind") {
                    model.set(key.clone(), row.label.clone());
                }
                out.children.push(clone);
            }
        }
        None => {
            for c in &tree.children {
                out.children.push(instantiate_rows(c, model, source));
            }
        }
    }
    out
}

/// The node with its scalar fields and NO children.
fn shallow(n: &UiNode) -> UiNode {
    UiNode {
        children: Vec::new(),
        ..n.clone()
    }
}

/// One row's clone of the prototype subtree with the placeholders substituted.
fn expand(proto: &UiNode, ordinal: usize, id: &str) -> UiNode {
    let sub = |s: &str| substitute(s, ordinal, id);
    let mut n = shallow(proto);
    n.id = sub(&proto.id);
    n.action = proto.action.as_deref().map(sub);
    n.visible_bind = proto.visible_bind.as_deref().map(sub);
    n.enabled_bind = proto.enabled_bind.as_deref().map(sub);
    n.nav_ordinal = proto.nav_ordinal.saturating_add(ordinal as u32);
    for v in n.props.values_mut() {
        if let Value::Text(s) = v {
            *s = sub(s);
        }
    }
    for c in &proto.children {
        n.children.push(expand(c, ordinal, id));
    }
    n
}

fn substitute(s: &str, ordinal: usize, id: &str) -> String {
    if !s.contains('{') {
        return s.to_string();
    }
    s.replace("{row}", &ordinal.to_string()).replace("{id}", id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    fn list() -> UiNode {
        let mut proto = UiNode {
            component: "radio".into(),
            id: "bones_{row}".into(),
            bind: Some("bone_sel".into()),
            tab_group: "controls".into(),
            nav_ordinal: 4,
            ..Default::default()
        };
        proto.props.insert("value".into(), text("{id}"));
        proto
            .props
            .insert("label_bind".into(), text("bones_{row}_label"));
        let mut list = UiNode {
            component: "list".into(),
            id: "bones".into(),
            bind: Some("bones_scroll".into()),
            children: vec![proto],
            ..Default::default()
        };
        list.props.insert(ROWS_FROM.into(), text("bones"));
        UiNode {
            component: "cell".into(),
            id: "root".into(),
            children: vec![list],
            ..Default::default()
        }
    }

    #[test]
    fn rows_expand_the_prototype_and_publish_their_labels() {
        let tree = list();
        let mut model = ValueMap::new();
        let out = instantiate_rows(&tree, &mut model, &|name| {
            (name == "bones").then(|| {
                vec![
                    Row::new("pelvis", "Pelvis"),
                    Row::new("spine_01", "Spine 1"),
                    Row::new("neck_01", "Neck"),
                ]
            })
        });
        let rows = &out.children[0].children;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].id, "bones_1");
        assert_eq!(rows[1].props.get("value"), Some(&text("spine_01")));
        assert_eq!(rows[1].bind.as_deref(), Some("bone_sel"));
        assert_eq!(rows[1].tab_group, "controls");
        assert_eq!(rows[2].nav_ordinal, 6);
        assert_eq!(model.text("bones_0_label"), Some("Pelvis"));
        assert_eq!(model.text("bones_2_label"), Some("Neck"));
        // The authored tree is untouched: still one prototype.
        assert_eq!(tree.children[0].children.len(), 1);
    }

    #[test]
    fn an_unknown_source_expands_to_no_rows() {
        let tree = list();
        let mut model = ValueMap::new();
        let out = instantiate_rows(&tree, &mut model, &|_| None);
        assert!(out.children[0].children.is_empty());
        assert_eq!(model.entries().count(), 0);
    }

    #[test]
    fn a_list_without_rows_from_keeps_its_authored_children() {
        let mut tree = list();
        tree.children[0].props.remove(ROWS_FROM);
        let mut model = ValueMap::new();
        let out = instantiate_rows(&tree, &mut model, &|_| Some(vec![Row::new("x", "X")]));
        assert_eq!(out.children[0].children.len(), 1);
        assert_eq!(out.children[0].children[0].id, "bones_{row}");
    }
}
