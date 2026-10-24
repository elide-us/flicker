//! **Data-driven rows** — a `list` authored with `rows_from: "<source>"` carries ONE
//! prototype child, and the scene expands it into one clone per row it publishes. A
//! `row` may repeat the same way (the shared file browser's breadcrumb: the path's
//! segments, one button each) — a repeater is a FLOW container, and the two differ only
//! in the axis they flow along.
//!
//! A `select` repeats its OPTIONS the same way: its LAST child is the prototype
//! `option`, the children authored before it stay as they are (a fixed leading choice —
//! None, Unbound, New), and each clone's numeric `value` is the prototype's `value` plus
//! the row's ordinal, so the options stay index-valued (an index is a number) and run
//! on from the authored ones. A dropdown over runtime data — material names, a graph's
//! nodes — is then authored data like any other list, and no scene builds options.
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
        // A repeater is a FLOW container — a `list` repeats down its scrolling column and
        // a `row` repeats across (the breadcrumb in the shared file browser is exactly
        // "the path's segments, one button each") — or a `select`, whose rows are its
        // options. Any other kind naming `rows_from` is authoring noise and is ignored.
        .filter(|_| matches!(tree.component.as_str(), "list" | "row" | "select"));
    match from {
        Some(name) => {
            // A flow repeats its FIRST child; a select its LAST, after the options
            // authored ahead of it, which it keeps.
            let select = tree.component == "select";
            let proto = if select {
                tree.children.last()
            } else {
                tree.children.first()
            };
            let Some(proto) = proto else {
                tracing::warn!(
                    "rows: {} `{}` has no prototype to expand",
                    tree.component,
                    tree.id
                );
                return out;
            };
            let Some(rows) = source(name) else {
                tracing::warn!(
                    "rows: {} `{}` names row source `{name}` the scene does not publish",
                    tree.component,
                    tree.id
                );
                return out;
            };
            let base = if select {
                out.children
                    .extend(tree.children[..tree.children.len() - 1].iter().cloned());
                match proto.props.get("value") {
                    Some(Value::Number(v)) => *v,
                    _ => {
                        tracing::warn!(
                            "rows: select `{}`'s prototype option has no numeric `value` — \
                             its options number from 0",
                            tree.id
                        );
                        0.0
                    }
                }
            } else {
                0.0
            };
            for (ordinal, row) in rows.iter().enumerate() {
                let mut clone = expand(proto, ordinal, &row.id);
                if select {
                    // The clone ROOT only, like the nav ordinal: an option is a leaf.
                    clone
                        .props
                        .insert("value".into(), Value::Number(base + ordinal as f64));
                }
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

    /// A select repeating its options: the authored leading option stays, the LAST child
    /// is the prototype, and each clone's `value` runs on from the prototype's — so the
    /// options are 0 (authored), 1, 2, … with their labels published like any row's.
    fn select(rows_from: &str) -> UiNode {
        let mut none = UiNode {
            component: "option".into(),
            ..Default::default()
        };
        none.props.insert("value".into(), Value::Number(0.0));
        none.props.insert("label".into(), text("$none"));
        let mut proto = UiNode {
            component: "option".into(),
            ..Default::default()
        };
        proto.props.insert("value".into(), Value::Number(1.0));
        proto
            .props
            .insert("label_bind".into(), text("field_opt{row}"));
        let mut sel = UiNode {
            component: "select".into(),
            id: "in0".into(),
            bind: Some("node_in0".into()),
            children: vec![none, proto],
            ..Default::default()
        };
        sel.props.insert(ROWS_FROM.into(), text(rows_from));
        UiNode {
            component: "cell".into(),
            children: vec![sel],
            ..Default::default()
        }
    }

    #[test]
    fn a_select_keeps_its_authored_options_and_numbers_its_rows_after_them() {
        let mut model = ValueMap::new();
        let out = instantiate_rows(&select("field"), &mut model, &|name| {
            (name == "field").then(|| {
                vec![
                    Row::new("ch1", "ch1 · Noise"),
                    Row::new("mix2", "mix2 · Blend"),
                ]
            })
        });
        let opts = &out.children[0].children;
        assert_eq!(opts.len(), 3, "the authored option plus one per row");
        let values: Vec<_> = opts.iter().map(|o| o.props.get("value").cloned()).collect();
        assert_eq!(
            values,
            vec![
                Some(Value::Number(0.0)),
                Some(Value::Number(1.0)),
                Some(Value::Number(2.0))
            ],
            "ascending numeric values, running on from the authored option"
        );
        assert_eq!(opts[0].props.get("label"), Some(&text("$none")));
        assert_eq!(opts[2].props.get("label_bind"), Some(&text("field_opt1")));
        assert_eq!(model.text("field_opt0"), Some("ch1 · Noise"));
        assert_eq!(model.text("field_opt1"), Some("mix2 · Blend"));
    }

    #[test]
    fn a_select_over_an_empty_source_keeps_only_its_authored_options() {
        let mut model = ValueMap::new();
        let out = instantiate_rows(&select("field"), &mut model, &|_| Some(Vec::new()));
        let opts = &out.children[0].children;
        assert_eq!(opts.len(), 1, "nothing to offer but the authored option");
        assert_eq!(opts[0].props.get("value"), Some(&Value::Number(0.0)));
        // And an unknown source is loud and offers nothing either — never the prototype.
        let out = instantiate_rows(&select("nope"), &mut ValueMap::new(), &|_| None);
        assert!(out.children[0].children.is_empty());
    }

    /// The flow repeaters are untouched by the select arm: a `list` still repeats its
    /// FIRST child, numbers nothing, and drops the rest.
    #[test]
    fn a_flow_repeater_still_repeats_its_first_child() {
        let mut tree = list();
        let mut extra = tree.children[0].children[0].clone();
        extra.id = "trailing".into();
        tree.children[0].children.push(extra);
        let mut model = ValueMap::new();
        let out = instantiate_rows(&tree, &mut model, &|_| {
            Some(vec![Row::new("a", "A"), Row::new("b", "B")])
        });
        let rows = &out.children[0].children;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "bones_0");
        assert_eq!(
            rows[1].props.get("value"),
            Some(&text("b")),
            "no renumbering"
        );
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
