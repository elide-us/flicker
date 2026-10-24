//! The inspector's gates — no GPU anywhere: the shared tree, its styles and stage, the facts
//! the host gathers, its Model, the walker's real layout and hit-test, the pad's real
//! events, the canvas's real pointer path, and the answer ladder are all reachable without
//! a device. Scratch package / staging roots stand in for the promotion path.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use flicker::render::{AttachmentFormat, Attachments, BloomPass, PassKind, Rect, Vec2};
use flicker::script::Value;
use flicker::ui::SceneDef;
use flicker_input_core::{ActionSignal, EventKind};
use flicker_input_router::{InputEvent, RouteCtx};
use flicker_materials::{JsonTableSource, Tables};
use flicker_texture::presets;

use super::*;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/data");

/// The shipped stringtable, so a walked frame carries resolved en-us text.
fn load_strings() {
    let table =
        std::fs::read_to_string(format!("{DATA}/stringtable.json")).expect("stringtable reads");
    strings::load_str(&table, "en-us");
}

/// The shipped material index, sorted by id.
fn rows() -> Vec<MaterialDef> {
    let mut rows = Tables::from_source(&JsonTableSource::new(DATA))
        .expect("the shipped material tables load")
        .materials()
        .to_vec();
    rows.sort_by_key(|m| m.id);
    rows
}

fn bands() -> Vec<MaterialBand> {
    JsonTableSource::new(DATA)
        .load_material_bands()
        .expect("the shipped bands read")
}

/// A scratch root, unique per process and per call (concurrent `cargo test` runs never
/// share one), removed by [`Scratch`]'s drop.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("flicker_insp_{name}_{}_{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// Granite — an assigned slot a bundle may define (the slot law).
const GRANITE: MaterialId = 10;
/// A FREE slot — no row of the index holds it.
const FREE: MaterialId = 67;

/// Write a bundle for `slot` under `root`: the seven maps at `px` (each its own colour, a
/// fine noise over it so a big map costs a real decode) and `recipe` as the document, in the
/// ONE layout `bundle_dir` spells, the recipe gz at rest.
fn write_bundle(root: &Path, slot: MaterialId, recipe: &str, px: u32) {
    let dir = flicker_content::bundle_dir(root, slot);
    std::fs::create_dir_all(&dir).unwrap();
    let seg = flicker_materials::slot_dir(slot);
    for (i, kind) in MapKind::ALL.into_iter().enumerate() {
        let c = (i as u8) * 30 + 20;
        let grain = |x: u32, y: u32| {
            ((x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) >> 7) as u8 & 3
        };
        image::RgbImage::from_fn(px, px, |x, y| {
            let g = grain(x, y);
            image::Rgb([c.saturating_add(g), (255 - c).saturating_sub(g), 128 + g])
        })
        .save(dir.join(format!("{seg}_{}.png", kind.role())))
        .expect("a map encodes");
    }
    flicker_content::package::write_text(&dir.join(format!("{seg}.texture.json")), recipe)
        .expect("the recipe writes");
}

/// A v2 graph document — what the MaterialBench commits.
fn v2_doc() -> String {
    serde_json::to_string(&MaterialGraph::from_recipe(&presets::granite())).unwrap()
}

/// A versionless v1 recipe document — the rack form the loader converts.
fn v1_doc() -> String {
    serde_json::to_string(&presets::granite()).unwrap()
}

fn gather(slot: MaterialId, package: &Path, staging: &Path) -> InspectorFacts {
    InspectorFacts::gather(slot, &rows(), &bands(), package, staging)
}

/// The long-lived pool the tests' inspectors borrow — the catalog's role at runtime.
fn pool() -> &'static WorkerPool {
    static POOL: OnceLock<WorkerPool> = OnceLock::new();
    POOL.get_or_init(|| WorkerPool::new(2))
}

/// The inspector on `slot` against scratch roots holding exactly what the test wrote.
fn open(slot: MaterialId, package: &Path, staging: &Path) -> MaterialInspector {
    MaterialInspector::new(gather(slot, package, staging), pool())
}

/// Whether `p` lies inside `r` — the canvas's own pick test.
fn contains(r: Rect, p: Vec2) -> bool {
    flicker_canvas::rect_contains(r, p)
}

/// One pointer sample at `screen`.
fn at(screen: Vec2, mouse: Vec2, pressed: bool) -> UiInput {
    UiInput {
        mouse,
        clicked: pressed,
        down: pressed,
        right_down: false,
        screen,
        wheel: 0.0,
        exclusive: false,
        motion: Default::default(),
    }
}

const HD: Vec2 = Vec2::new(1920.0, 1080.0);
const OFF: Vec2 = Vec2::new(-9.0, -9.0);
const DT: Duration = Duration::from_millis(16);

fn centre(r: Rect) -> Vec2 {
    r.pos + r.size * 0.5
}

/// The shared file's RAW document.
fn raw_doc() -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(INSPECTOR_JSON).expect("the shared tree parses")
}

/// Walk the shared tree's RAW JSON.
fn raw_tree() -> serde_json::Value {
    raw_doc()["tree"].clone()
}

fn raw_nodes<'v>(v: &'v serde_json::Value, out: &mut Vec<&'v serde_json::Value>) {
    out.push(v);
    if let Some(kids) = v.get("children").and_then(|c| c.as_array()) {
        for k in kids {
            raw_nodes(k, out);
        }
    }
}

fn flatten<'a>(node: &'a UiNode, out: &mut Vec<&'a UiNode>) {
    out.push(node);
    for kid in &node.children {
        flatten(kid, out);
    }
}

fn press<'a>(signal: ActionSignal, raw: &'a InputState) -> InputEvent<'a> {
    InputEvent::new(signal, EventKind::Press, InputContext::Menu, raw)
}

// ── the tree ─────────────────────────────────────────────────────────────────

/// THE TREE IS A SHARED MODAL WITH THE HOST'S EXIT: it loads through the shell's one
/// parse path, so its root declares the host's back-out whatever the file says; the
/// Close button fires that same name; the slab is `dismissable` and authors no bind that
/// could hold it shut; and the shell names THIS scene as the tree's host, so the param
/// seam refuses it by name rather than hosting it as a trap (incident B89FAC21).
#[test]
fn the_tree_is_a_shared_modal_carrying_the_hosts_exit() {
    let none = scratch("exit_none");
    let insp = open(GRANITE, &none.0, &none.0);
    let tree = insp.tree();
    assert_eq!(tree.component, "surface", "a full-bleed surface root");
    assert_eq!(
        insp.intents().result_for(ActionSignal::Cancel),
        Some(MODAL_CANCEL),
        "Esc / pad-B fire the host's exit"
    );

    let raw = raw_tree();
    let mut all = Vec::new();
    raw_nodes(&raw, &mut all);
    let slabs: Vec<_> = all
        .iter()
        .filter(|n| n.get("component").and_then(|c| c.as_str()) == Some("popup_panel"))
        .collect();
    assert_eq!(slabs.len(), 1, "one slab");
    assert_eq!(
        slabs[0].get("dismissable").and_then(|d| d.as_bool()),
        Some(true),
        "the slab is authored dismissable"
    );
    assert!(
        slabs[0].get("dismissable_bind").is_none(),
        "nothing here may hold the slab shut"
    );
    assert_eq!(
        slabs[0].get("title_bind").and_then(|t| t.as_str()),
        Some(TITLE)
    );
    assert!(flicker::ui::popup_dismissable(tree, insp.model()));
    // The shell's own never-a-trap predicate, the one its registry is held to.
    let (traps, binds) = flicker_shell::modal_trap_problems(INSPECTOR_ID, &raw_doc());
    assert!(traps.is_empty(), "{traps:#?}");
    assert!(binds.is_empty(), "nothing binds the toggle: {binds:?}");

    let close = all
        .iter()
        .find(|n| n.get("id").and_then(|i| i.as_str()) == Some("modal_close"))
        .expect("the tree authors its Close button");
    assert_eq!(
        close.get("action").and_then(|a| a.as_str()),
        Some(MODAL_CANCEL),
        "Close is the same exit Esc and pad-B take — one way out"
    );

    assert_eq!(
        flicker_shell::modal_host_of(INSPECTOR_ID),
        Some("flicker_materialcatalog::MaterialInspector"),
        "the shell names this scene as the tree's host"
    );
    assert!(
        !flicker_shell::param_driven_modals().contains(&INSPECTOR_ID),
        "the param seam never offers it"
    );
}

/// The VOCABULARY and STRINGS gates — the ones the shell holds every registered shared
/// tree to, held here for the tree this crate hosts: only kinds the engine knows, and
/// every display literal a `$token`.
#[test]
fn the_tree_names_only_known_kinds_and_ships_no_display_literals() {
    // The shell's own predicate, over the tree as HOSTED — the node list expanded.
    let package = scratch("vocab");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    let insp = open(GRANITE, &package.0, &package.0);
    let problems = flicker_shell::modal_vocabulary_problems(INSPECTOR_ID, insp.tree());
    assert!(problems.is_empty(), "{problems:#?}");
}

/// PAD-REACHABILITY — the shell's gate over its registered trees, held here for this one:
/// every authored button carries an `id`, a `tab_group` and a `nav_ordinal` (read from the
/// RAW file: a missing ordinal parses to 0), and the walker's own nav flatten reaches them.
#[test]
fn every_button_is_pad_reachable() {
    let raw = raw_tree();
    let mut all = Vec::new();
    raw_nodes(&raw, &mut all);
    let buttons: Vec<_> = all
        .iter()
        .filter(|n| n.get("component").and_then(|c| c.as_str()) == Some("button"))
        .collect();
    assert_eq!(
        buttons.len(),
        3,
        "the body toggle, the node row's prototype and Close"
    );
    // The shell's own predicate over the RAW file, then the id this host needs as well.
    let problems = flicker_shell::modal_pad_problems(INSPECTOR_ID, &raw_doc());
    assert!(problems.is_empty(), "{problems:#?}");
    for b in &buttons {
        assert!(b.get("id").is_some(), "button {b} authors no `id`");
    }
    // The walker reaches every control the host EXPANDED: the toggle, the zoom slider under
    // it, one row per card, and Close — in that order around the `modal` ring.
    let package = scratch("pad");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    let insp = open(GRANITE, &package.0, &package.0);
    let cards = insp.canvas_keys().len();
    let mut ring: Vec<_> = focusables_of(insp.tree(), insp.model())
        .into_iter()
        .filter(|f| f.group == "modal")
        .collect();
    ring.sort_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.id.cmp(&b.id)));
    let ids: Vec<String> = ring.into_iter().map(|f| f.id).collect();
    let mut want = vec!["insp_body".to_string(), ZOOM.to_string()];
    want.extend((0..cards).map(|i| format!("insp_nl_{i}")));
    want.push("modal_close".to_string());
    assert_eq!(ids, want, "toggle, zoom, every row, Close — the pad's ring");
}

/// Every style path the tree authors resolves to a BLOCK, and every text `color` to a
/// COLOUR, in the styles the host renders with (the carrier ⊕ the tree's own blocks) — a
/// path that resolves to nothing draws compiled defaults silently.
#[test]
fn every_style_and_colour_path_resolves() {
    let none = scratch("styles");
    let insp = open(GRANITE, &none.0, &none.0);
    let lookup = |path: &str| -> Option<&serde_json::Value> {
        path.split('.').try_fold(insp.styles(), |v, k| v.get(k))
    };
    let mut all = Vec::new();
    flatten(insp.tree(), &mut all);
    let mut checked = 0;
    for n in &all {
        if let Some(Value::Text(path)) = n.props.get("style") {
            assert!(
                lookup(path).is_some_and(|v| v.is_object()),
                "{}: style `{path}` resolves to no block",
                n.component
            );
            checked += 1;
        }
        if let Some(Value::Text(path)) = n.props.get("color") {
            assert!(
                lookup(path).is_some_and(|v| v.as_array().is_some_and(|a| a.len() == 4)),
                "{}: color `{path}` resolves to no rgba",
                n.component
            );
            checked += 1;
        }
    }
    assert!(checked > 40, "the sweep saw the authored paths ({checked})");
    // The slab's chrome rides the carrier.
    assert!(lookup("modal.panel").is_some_and(|v| v.is_object()));
}

/// EVERY BIND IS PUBLISHED: each Model key the tree binds — a title, a text, a label, a
/// texture — carries a value in the host's Model, for an assigned slot with a bundle, one
/// with none, and a free slot. The one deliberate absence is the free slot's swatch
/// texture: its sprite must draw NOTHING, which is what an unpublished `tex` draws.
#[test]
fn every_bind_is_published() {
    /// Every Model key a walked tree binds — the EXPANDED tree, so a row's `{row}` is
    /// already its number.
    fn binds_of(tree: &UiNode) -> Vec<String> {
        let mut all = Vec::new();
        flatten(tree, &mut all);
        let mut binds: Vec<String> = Vec::new();
        for n in &all {
            binds.extend(n.bind.iter().cloned());
            binds.extend(n.visible_bind.iter().cloned());
            binds.extend(n.enabled_bind.iter().cloned());
            for (k, v) in &n.props {
                if let (true, Value::Text(key)) = (k.ends_with("_bind"), v) {
                    binds.push(key.clone());
                }
            }
        }
        binds.sort();
        binds.dedup();
        binds
    }

    let package = scratch("binds_pkg");
    let none = scratch("binds_none");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    for (case, slot, root) in [
        ("an assigned slot with a bundle", GRANITE, &package.0),
        ("an assigned slot with none", GRANITE, &none.0),
        ("a free slot", FREE, &none.0),
    ] {
        let insp = open(slot, root, &none.0);
        let binds = binds_of(insp.tree());
        assert!(
            binds.len() > 20,
            "{case}: the tree binds its facts: {binds:?}"
        );
        if slot == GRANITE && root == &package.0 {
            assert!(
                binds.iter().any(|b| b == "insp_nl0_label"),
                "{case}: the node list's rows bind their labels: {binds:?}"
            );
        }
        for key in &binds {
            let published = insp.model().get(key).is_some();
            if slot == FREE && key == SWATCH_TEX {
                assert!(
                    !published,
                    "{case}: no swatch texture, so the sprite draws nothing"
                );
            } else {
                assert!(
                    published,
                    "{case}: the tree binds `{key}` and nothing publishes it"
                );
            }
        }
    }
}

// ── the stage ────────────────────────────────────────────────────────────────

fn bloom_of(def: &StageDef) -> BloomPass {
    let blooms: Vec<BloomPass> = def
        .recipe()
        .iter()
        .filter_map(|p| match &p.kind {
            PassKind::Bloom(b) => Some(*b),
            _ => None,
        })
        .collect();
    assert_eq!(blooms.len(), 1, "one bloom: {blooms:?}");
    blooms[0]
}

/// THE STAGE RESOLVES like the other two lit stages: through the one stage compiler and
/// the shared checks (`sample_stage_check` = the sample's own problems + the HDR resolve),
/// with the `material` layer and nothing else, a framing, a transparent clear, HDR + depth,
/// `bloom` after the scene pass, exactly ONE `tonemap_grade` resolving last — and its bloom
/// and its emissive gain EQUAL to the catalog's `mc_sample` and the MaterialBench's
/// `materialbench_lit`, so a glow burns alike in all three.
#[test]
fn the_stage_blooms_and_burns_exactly_as_the_catalog_and_the_bench_do() {
    let none = scratch("stage");
    let insp = open(GRANITE, &none.0, &none.0);
    let (def, problems) = flicker::ui::compile_stage(insp.styles(), INSPECTOR_STAGE)
        .expect("the tree authors its stage");
    assert!(problems.is_empty(), "authoring problems: {problems:?}");
    assert!(sample_stage_check(&def, INSPECTOR_STAGE).is_empty());
    assert!(insp.stage().is_some(), "the host compiled and kept it");
    assert!(def.has_layer("material"));
    assert!(def
        .layers_outside(flicker_materialview::SAMPLE_LAYERS)
        .is_empty());
    assert!(def.camera.is_some(), "framed");
    assert_eq!(def.clear, Some([0.0; 4]), "the well is the backdrop");
    assert_eq!(
        def.attachments.get(Attachments::HDR).map(|a| a.format),
        Some(AttachmentFormat::Rgba16f)
    );
    assert!(def.attachments.get(Attachments::DEPTH).is_some());
    let (order, cyclic) = def.pass_order();
    assert!(!cyclic);
    let kinds: Vec<&str> = order.iter().map(|&i| def.recipe()[i].kind.kind()).collect();
    let at = |k: &str| kinds.iter().position(|x| *x == k).expect("the pass runs");
    assert!(at("scene") < at("bloom"), "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| **k == "tonemap_grade").count(), 1);
    assert_eq!(kinds.last(), Some(&"tonemap_grade"), "{kinds:?}");

    let catalog = SceneDef::parse(
        "materialcatalog",
        include_str!("../../../../../content/sensorium/scenes/materialcatalog.scene.json"),
    )
    .expect("the catalog parses");
    let catalog_styles = flicker::ui::load_shared_styles(catalog.styles.as_ref());
    let (mc, _) = flicker::ui::compile_stage(&catalog_styles, crate::SAMPLE_STAGE_SOURCE)
        .expect("the catalog authors mc_sample");
    let bench = SceneDef::parse(
        "materialbench",
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/sensorium/scenes/materialbench.scene.json"
        ))
        .expect("the bench's scene reads"),
    )
    .expect("the bench's scene parses");
    let bench_styles = flicker::ui::load_shared_styles(bench.styles.as_ref());
    let (lit, _) = flicker::ui::compile_stage(&bench_styles, "materialbench_lit")
        .expect("the bench authors its lit stage");
    assert_eq!(bloom_of(&def), bloom_of(&mc), "blooms as the catalog does");
    assert_eq!(bloom_of(&def), bloom_of(&lit), "blooms as the bench does");
    assert!(def.emissive_gain >= 1.0 && def.emissive_gain.is_finite());
    assert_eq!(def.emissive_gain, mc.emissive_gain, "the catalog's gain");
    assert_eq!(def.emissive_gain, lit.emissive_gain, "the bench's gain");
}

// ── the facts ────────────────────────────────────────────────────────────────

fn text<'m>(insp: &'m MaterialInspector, key: &str) -> &'m str {
    insp.model()
        .text(key)
        .unwrap_or_else(|| panic!("`{key}` is published as text"))
}

/// THE HEADER SAYS WHAT THE PROMOTION PATH HOLDS — the package first, then staging, else
/// nowhere — and that bundle's facts: the declared recipe version, the node count, the map
/// set, the map size, the bytes on disk and the newest file time. A recipe that does not
/// load is said so, never shown as a blank graph.
#[test]
fn the_header_says_what_the_promotion_path_holds() {
    load_strings();
    let package = scratch("held_pkg");
    let staging = scratch("held_stg");
    let none = scratch("held_none");

    // PACKAGE — a v2 graph, and the package wins over a staged twin.
    write_bundle(&package.0, GRANITE, &v2_doc(), 16);
    write_bundle(&staging.0, GRANITE, &v1_doc(), 8);
    let insp = open(GRANITE, &package.0, &staging.0);
    let facts = insp.facts();
    assert_eq!(facts.held, Held::Package, "the package is asked first");
    assert!(matches!(facts.recipe, Recipe::Loaded { version: 2, .. }));
    assert_eq!(text(&insp, "insp_pkg_state"), "$insp_state_package");
    assert_eq!(text(&insp, "insp_pkg_note"), "$insp_note_package");
    assert_eq!(text(&insp, "insp_pkg_version"), "2");
    let nodes = facts.graph().expect("the graph loaded").nodes.len();
    assert_eq!(text(&insp, "insp_pkg_nodes"), nodes.to_string());
    assert_eq!(
        text(&insp, "insp_pkg_maps"),
        "BaseColor · Normal · Roughness · Metallic · AO · Height · Emit",
        "the seven roles present"
    );
    assert_eq!(text(&insp, "insp_pkg_size"), "16 × 16");
    let dir = flicker_content::bundle_dir(&package.0, GRANITE);
    let on_disk: u64 = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert_eq!(
        facts.bytes, on_disk,
        "every file at rest, the gz recipe included"
    );
    assert_eq!(text(&insp, "insp_pkg_bytes"), human_bytes(on_disk));
    let stamp = text(&insp, "insp_pkg_modified");
    assert!(
        stamp.len() == 16 && stamp.as_bytes()[4] == b'-' && stamp.as_bytes()[13] == b':',
        "a UTC stamp: {stamp}"
    );
    assert_eq!(
        text(&insp, GRAPH_NOTE),
        "",
        "a graph to show, no note over it"
    );

    // STAGING — a versionless v1 recipe reads as v1, through the same one loader.
    let insp = open(GRANITE, &none.0, &staging.0);
    assert_eq!(insp.facts().held, Held::Staging);
    assert_eq!(text(&insp, "insp_pkg_state"), "$insp_state_staging");
    assert_eq!(text(&insp, "insp_pkg_note"), "$insp_note_staging");
    assert_eq!(text(&insp, "insp_pkg_version"), "1");
    assert_eq!(text(&insp, "insp_pkg_size"), "8 × 8");
    assert!(
        insp.facts().graph().is_some(),
        "a v1 recipe converts to a graph"
    );

    // NOWHERE — every bundle fact is a dash, the note says nothing was made, no graph.
    let insp = open(GRANITE, &none.0, &none.0);
    assert_eq!(insp.facts().held, Held::Nowhere);
    assert!(matches!(insp.facts().recipe, Recipe::Absent));
    assert_eq!(text(&insp, "insp_pkg_state"), "$insp_state_none");
    assert_eq!(text(&insp, "insp_pkg_note"), "$insp_note_none");
    for key in [
        "insp_pkg_version",
        "insp_pkg_nodes",
        "insp_pkg_maps",
        "insp_pkg_size",
        "insp_pkg_bytes",
        "insp_pkg_modified",
    ] {
        assert_eq!(text(&insp, key), NONE, "{key} reads a dash with no bundle");
    }
    assert_eq!(text(&insp, GRAPH_NOTE), "$insp_graph_none");
    assert!(insp.canvas_keys().is_empty(), "no graph, no cards");
    assert!(!insp.decoding(), "nothing to decode");

    // INVALID — a bundle whose recipe does not load is named, not shown blank.
    let broken = scratch("held_broken");
    write_bundle(&broken.0, GRANITE, "{ not a material", 8);
    let insp = open(GRANITE, &broken.0, &none.0);
    assert!(matches!(insp.facts().recipe, Recipe::Invalid));
    assert_eq!(text(&insp, "insp_pkg_state"), "$insp_state_package");
    assert_eq!(text(&insp, "insp_pkg_note"), "$insp_note_invalid");
    assert_eq!(text(&insp, "insp_pkg_version"), NONE);
    assert_eq!(text(&insp, GRAPH_NOTE), "$insp_graph_invalid");
    assert!(insp.canvas_keys().is_empty());
}

/// THE FOOTER IS THE DEFINITION: the slot's index row — id (decimal and hex), name,
/// category, render class (a token), colour, what it represents, its signature, its traits —
/// and the band the id is filed under. A FREE slot says it is unassigned, dashes the rest,
/// keeps its band, and publishes no swatch texture.
#[test]
fn the_footer_is_the_definition() {
    load_strings();
    let none = scratch("def");
    let insp = open(GRANITE, &none.0, &none.0);
    let granite = rows().into_iter().find(|r| r.id == GRANITE).unwrap();
    assert_eq!(
        text(&insp, TITLE),
        granite.name,
        "the slab is titled with the name"
    );
    assert_eq!(text(&insp, "insp_def_id"), "10 · 0x0A");
    assert_eq!(text(&insp, "insp_def_name"), granite.name);
    assert_eq!(text(&insp, "insp_def_category"), granite.category);
    assert_eq!(text(&insp, "insp_def_class"), "$insp_class_hard_edge");
    assert_eq!(text(&insp, "insp_def_colour"), hex(granite.color));
    assert_eq!(
        text(&insp, "insp_def_represents"),
        granite.represents.join(", ")
    );
    assert_eq!(
        text(&insp, "insp_def_signature"),
        granite.signature.join(" · ")
    );
    assert_eq!(
        text(&insp, "insp_def_hardness"),
        num(f64::from(granite.hardness))
    );
    assert_eq!(
        text(&insp, "insp_def_density"),
        num(f64::from(granite.density_g_cm3))
    );
    let band = band_of(&bands(), GRANITE)
        .expect("10 is filed")
        .label
        .clone();
    assert_eq!(text(&insp, "insp_def_band"), band);
    assert!(band.starts_with("10-19"));
    assert_eq!(
        insp.model().number(SWATCH_TEX),
        Some(Theme::TEXTURE_NAMES.len() as f64),
        "the swatch sits after the theme's own textures in the HUD's table"
    );

    // A FREE slot.
    let free = open(FREE, &none.0, &none.0);
    assert!(free.facts().row.is_none());
    assert_eq!(text(&free, TITLE), strings::resolve(UNASSIGNED));
    assert_eq!(text(&free, "insp_def_name"), UNASSIGNED);
    assert_eq!(text(&free, "insp_def_category"), NONE);
    assert_eq!(text(&free, "insp_def_class"), NONE);
    assert_eq!(text(&free, "insp_def_id"), format!("{FREE} · 0x{FREE:02X}"));
    assert_eq!(
        text(&free, "insp_def_band"),
        band_of(&bands(), FREE).expect("every id is filed").label
    );
    assert!(free.model().get(SWATCH_TEX).is_none());
    assert_eq!(
        text(&free, GRAPH_NOTE),
        "$insp_graph_none",
        "no graph, said so"
    );

    // Every render class has its token.
    for (class, token) in [
        (Some(RenderClass::Blendable), "$insp_class_blendable"),
        (Some(RenderClass::HardEdge), "$insp_class_hard_edge"),
        (Some(RenderClass::Translucent), "$insp_class_translucent"),
        (Some(RenderClass::Emissive), "$insp_class_emissive"),
        (None, NONE),
    ] {
        assert_eq!(class_token(class), token);
        assert!(
            !strings::resolve(token).starts_with('$'),
            "{token} is seeded"
        );
    }
}

/// The two read-out formats are honest: a number at its shortest reading, a file time as a
/// UTC civil date (leap day included).
#[test]
fn numbers_and_times_read_honestly() {
    assert_eq!(num(7.0), "7");
    assert_eq!(num(0.4), "0.4");
    assert_eq!(num(2.65), "2.65");
    assert_eq!(num(0.0), "0");
    assert_eq!(num(-0.0), "0");
    let t = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
    assert_eq!(utc_stamp(t(0)).as_deref(), Some("1970-01-01 00:00"));
    assert_eq!(
        utc_stamp(t(951_782_400)).as_deref(),
        Some("2000-02-29 00:00")
    );
    assert_eq!(
        utc_stamp(t(1_000_000_000)).as_deref(),
        Some("2001-09-09 01:46")
    );
    assert_eq!(
        utc_stamp(t(1_792_607_669)).as_deref(),
        Some("2026-10-21 18:34")
    );
}

// ── the walked modal ─────────────────────────────────────────────────────────

/// THE SLAB LAYS OUT WHOLE at 1080p and at 1600×900 (the shell gates' size): inside the
/// screen, every panel and both buttons with real extent inside the slab, and both
/// surfaces reserved — the lit sample LIVE under the inspector's stage, the canvas filled by
/// the host. And every token it draws resolves.
#[test]
fn the_slab_lays_out_whole_and_every_token_resolves() {
    load_strings();
    let package = scratch("layout");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    for screen in [HD, Vec2::new(1600.0, 900.0)] {
        for slot in [GRANITE, FREE] {
            let mut insp = open(slot, &package.0, &package.0);
            let frame = insp.frame(DT, &at(screen, OFF, false), false);
            let popup = frame.rect("popup").expect("the slab is laid out");
            assert!(
                popup.pos.x >= 0.0
                    && popup.pos.y >= 0.0
                    && popup.pos.x + popup.size.x <= screen.x
                    && popup.pos.y + popup.size.y <= screen.y,
                "the slab fits {screen:?}: {popup:?}"
            );
            for id in [
                "insp_pkg",
                "insp_def",
                "insp_body",
                "modal_close",
                "insp_graph",
            ] {
                let r = frame.rect(id).unwrap_or_else(|| panic!("{id} is laid out"));
                assert!(
                    r.size.x > 20.0 && r.size.y > 20.0,
                    "{id} has real extent: {r:?}"
                );
                assert!(
                    r.pos.x >= popup.pos.x - 0.5
                        && r.pos.y >= popup.pos.y - 0.5
                        && r.pos.x + r.size.x <= popup.pos.x + popup.size.x + 0.5
                        && r.pos.y + r.size.y <= popup.pos.y + popup.size.y + 0.5,
                    "{id} sits inside the slab"
                );
            }
            let sample = frame.surface(SAMPLE_SLOT).expect("the sample's seat");
            assert_eq!(sample.source, INSPECTOR_STAGE);
            assert_eq!(sample.rate, flicker::render::Rate::Live, "a live turntable");
            assert!(sample.w > 200.0 && sample.h > 200.0, "a stage, not a chip");
            let canvas = frame.surface(CANVAS_SLOT).expect("the canvas's seat");
            assert!(canvas.w > 400.0 && canvas.h > 200.0);
            let drawn: Vec<&str> = insp
                .hud()
                .iter()
                .filter_map(|c| match c {
                    HudCommand::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(
                !drawn.iter().any(|t| t.starts_with('$')),
                "an unresolved token on the inspector: {drawn:?}"
            );
            let title = strings::resolve(text(&insp, TITLE)).into_owned();
            assert!(drawn.contains(&title.as_str()), "the slab draws its title");
            // Both buttons draw their labels (the shell's render gate, for this tree).
            for label in ["$modal_lbl_close", "$mtl_body_sphere"] {
                let want = strings::resolve(label);
                assert!(
                    drawn.contains(&want.as_ref()),
                    "a button draws `{want}`: {drawn:?}"
                );
            }
        }
    }
}

/// THE WAY OUT, through the channels a player uses (rule 8634C200): a POINTER press on
/// Close at its laid-out rect fires the host's exit and the answer closes with the host's
/// `cancelled`; the PAD's Cancel through the real walker fires the same name; and a pad
/// Confirm on Close, reached with the d-pad from the seeded focus, does too.
#[test]
fn close_and_cancel_both_close_with_the_hosts_answer() {
    let none = scratch("close");
    let closes = |t: Transition| match t {
        Transition::CloseModal {
            modal,
            result,
            payload,
        } => modal == INSPECTOR_ID && result == flicker_shell::MODAL_CANCELLED && payload.is_none(),
        _ => false,
    };

    // The pointer, at the button's laid-out rect.
    let mut insp = open(GRANITE, &none.0, &none.0);
    let close = insp
        .frame(DT, &at(HD, OFF, false), false)
        .rect("modal_close")
        .expect("Close is laid out");
    let frame = insp.frame(DT, &at(HD, centre(close), true), false);
    assert!(
        frame.results.is_on(MODAL_CANCEL),
        "the click fires the exit"
    );
    assert!(closes(insp.answer(&frame.results)), "and the host closes");

    // The pad's Cancel, through the real walker.
    let mut insp = open(GRANITE, &none.0, &none.0);
    insp.frame(DT, &at(HD, OFF, false), false);
    let raw = InputState::new();
    let mut rc = RouteCtx::new();
    let (tree, model, intents) = (
        insp.tree().clone(),
        insp.model().clone(),
        insp.intents().clone(),
    );
    let fired = {
        let mut h = WalkerHandler::hud(insp.ui_state(), false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
        h.handle(&press(ActionSignal::Cancel, &raw), &mut rc);
        h.take_fired()
    };
    assert_eq!(
        fired,
        vec![MODAL_CANCEL.to_string()],
        "Cancel fires the exit"
    );
    let mut results = ValueMap::new();
    results.set(MODAL_CANCEL, true);
    assert!(closes(insp.answer(&results)));

    // The pad's Confirm on Close: the focus was seeded on the first control (the body
    // toggle); the d-pad steps to Close — past the zoom slider under the toggle, whose own
    // axis is Left / Right, so Down moves on — and Confirm activates it.
    assert_eq!(insp.ui_state().focused(), Some("insp_body"), "seeded focus");
    let fired = {
        let mut h = WalkerHandler::hud(insp.ui_state(), false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        h.handle(&press(ActionSignal::Confirm, &raw), &mut rc);
        h.take_fired()
    };
    assert!(
        fired.contains(&MODAL_CANCEL.to_string()),
        "Confirm on Close: {fired:?}"
    );
}

/// THE BODY TOGGLE swaps Sphere and Plane — a pointer press at its rect fires its action,
/// the answer turns the sample's body, and the label reads the MaterialBench's own tokens.
#[test]
fn the_body_toggle_swaps_sphere_and_plane() {
    load_strings();
    let none = scratch("body");
    let mut insp = open(GRANITE, &none.0, &none.0);
    assert_eq!(insp.body(), Body::Sphere);
    assert_eq!(text(&insp, BODY_LABEL), "$mtl_body_sphere");
    let body = insp
        .frame(DT, &at(HD, OFF, false), false)
        .rect("insp_body")
        .expect("the toggle is laid out");
    let frame = insp.frame(DT, &at(HD, centre(body), true), false);
    assert!(frame.results.is_on(BODY_ACTION));
    assert!(
        matches!(insp.answer(&frame.results), Transition::None),
        "it does not close"
    );
    assert_eq!(insp.body(), Body::Plane);
    assert_eq!(text(&insp, BODY_LABEL), "$mtl_body_plane");
    assert!(!strings::resolve("$mtl_body_plane").starts_with('$'));
}

// ── the sample's zoom (Aaron 2026-10-23: "zoom in on the sphere view") ───────────

/// The zoom slider's numbers as the RAW shared file authors them.
fn authored_zoom() -> ZoomDial {
    let raw = raw_tree();
    let mut all = Vec::new();
    raw_nodes(&raw, &mut all);
    let slider = all
        .into_iter()
        .find(|n| n.get("id").and_then(|i| i.as_str()) == Some(ZOOM))
        .expect("the tree authors the zoom slider");
    let num = |key: &str| {
        slider[key]
            .as_f64()
            .unwrap_or_else(|| panic!("the zoom slider authors `{key}`")) as f32
    };
    ZoomDial {
        min: num("min"),
        max: num("max"),
        step: num("step"),
    }
}

/// One frame with a wheel notch at `mouse`.
fn wheel_at(mouse: Vec2, wheel: f32) -> UiInput {
    UiInput {
        wheel,
        ..at(HD, mouse, false)
    }
}

/// THE SLIDER ZOOMS THE SAMPLE, through the walker's real layout and hit-test: the host's
/// dial IS the tree's own slider (no param repeats its range); the slider sits under the body
/// toggle in the sample's column, its caption resolved; a press on its rail's far end and the
/// release — the walker's drag, committed on release — zoom the sample to the top of the
/// range, a press mid-rail to the middle (snapped to the step), and the Model echoes each as a
/// NUMBER; idle frames (the slider's echo) never move it, and the body toggle keeps it.
#[test]
fn the_zoom_slider_moves_the_sample_and_the_model_echoes_it() {
    load_strings();
    let none = scratch("zoom_slider");
    let mut insp = open(GRANITE, &none.0, &none.0);
    let dial = authored_zoom();
    assert_eq!(insp.zoom, dial, "the dial is the tree's own slider");
    assert_eq!(
        insp.sample.zoom, 1.0,
        "the sample opens on the stage's framing"
    );
    assert_eq!(
        insp.model().get(ZOOM),
        Some(&Value::Number(1.0)),
        "a number"
    );
    let frame = insp.frame(DT, &at(HD, OFF, false), false);
    let slider = frame.rect(ZOOM).expect("the zoom slider is laid out");
    let (column, body) = (
        frame.rect("insp_sample_col").expect("the sample's column"),
        frame.rect("insp_body").expect("the body toggle"),
    );
    assert!(
        slider.size.x > 200.0 && slider.size.y >= 28.0,
        "a real slider: {slider:?}"
    );
    assert!(
        slider.pos.y >= body.pos.y + body.size.y
            && contains(column, slider.pos)
            && contains(column, slider.pos + slider.size),
        "under the body toggle, in the sample's column: {slider:?} in {column:?}"
    );
    let caption = strings::resolve("$insp_zoom").into_owned();
    assert!(!caption.starts_with('$'), "the caption resolves");
    assert!(
        insp.hud()
            .iter()
            .any(|c| matches!(c, HudCommand::Text { text, .. } if *text == caption)),
        "the slider draws its caption `{caption}`"
    );

    // The rail is the slider less its readout column (`value_w` 46), its caption above it.
    let rail_y = slider.pos.y + slider.size.y * 0.75;
    let press = |insp: &mut MaterialInspector, x: f32| {
        insp.frame(DT, &at(HD, Vec2::new(x, rail_y), true), false);
        insp.frame(DT, &at(HD, Vec2::new(x, rail_y), false), false);
    };
    press(&mut insp, slider.pos.x + slider.size.x - 46.0 - 1.0);
    assert_eq!(
        insp.sample.zoom, dial.max,
        "the far end is the closest framing"
    );
    assert_eq!(insp.model().number(ZOOM), Some(f64::from(dial.max)));
    press(&mut insp, slider.pos.x + (slider.size.x - 46.0) * 0.5);
    let middle = dial.min + (dial.max - dial.min) * 0.5;
    assert_eq!(insp.sample.zoom, middle, "mid-rail, snapped to the step");
    for _ in 0..3 {
        insp.frame(DT, &at(HD, OFF, false), false);
    }
    assert_eq!(
        insp.sample.zoom, middle,
        "the slider's echo is never an edit"
    );
    assert_eq!(
        insp.model().get(ZOOM),
        Some(&Value::Number(f64::from(middle)))
    );

    let mut results = ValueMap::new();
    results.set(BODY_ACTION, true);
    insp.answer(&results);
    assert_eq!(insp.body(), Body::Plane);
    assert_eq!(insp.sample.zoom, middle, "the body toggle keeps the zoom");
}

/// THE WHEEL OVER THE SAMPLE ZOOMS, through the walker's real pointer barrier: one notch is
/// one AUTHORED step (the slider's own), notches add, the zoom stops at either end of the
/// authored range, and the slider follows through the Model.
#[test]
fn a_wheel_notch_over_the_sample_zooms_by_the_authored_step_and_clamps() {
    let none = scratch("zoom_wheel");
    let mut insp = open(GRANITE, &none.0, &none.0);
    let dial = authored_zoom();
    let seat = insp
        .frame(DT, &at(HD, OFF, false), false)
        .surface(SAMPLE_SLOT)
        .map(graph_view::seat_of)
        .expect("the sample is seated");
    let over = centre(seat);
    let frame = insp.frame(DT, &wheel_at(over, 1.0), false);
    assert!(
        frame.surface_pointer(SAMPLE_SLOT).is_some(),
        "the walker hands the sample its pointer"
    );
    assert_eq!(insp.sample.zoom, 1.0 + dial.step, "one notch, one step");
    insp.frame(DT, &wheel_at(over, 2.0), false);
    assert_eq!(insp.sample.zoom, 1.0 + 3.0 * dial.step, "notches add");
    assert_eq!(
        insp.model().number(ZOOM),
        Some(f64::from(1.0 + 3.0 * dial.step)),
        "the slider follows through the Model"
    );
    insp.frame(DT, &at(HD, OFF, false), false);
    assert_eq!(
        insp.sample.zoom,
        1.0 + 3.0 * dial.step,
        "its echo is no edit"
    );
    for _ in 0..64 {
        insp.frame(DT, &wheel_at(over, 1.0), false);
    }
    assert_eq!(insp.sample.zoom, dial.max, "clamped at the closest");
    insp.frame(DT, &wheel_at(over, -1.0), false);
    assert_eq!(insp.sample.zoom, dial.max - dial.step, "and back one step");
    for _ in 0..64 {
        insp.frame(DT, &wheel_at(over, -1.0), false);
    }
    assert_eq!(insp.sample.zoom, dial.min, "clamped at the farthest");
}

/// THE WHEEL IS THE SAMPLE'S ONLY OVER ITS SEAT: over the graph canvas a notch zooms the
/// graph and never the sample; over the zoom slider under the seat it zooms nothing; and a
/// frame whose pointer an open menu owns (`popup_hit` — the modal's own answer, which the
/// host defers to like every graph host) gives the seat's notch to nobody, where the same
/// notch, no menu, zooms.
#[test]
fn the_sample_zoom_wheel_never_reaches_through_a_menu_or_off_its_seat() {
    let package = scratch("zoom_gate");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    let mut insp = open(GRANITE, &package.0, &package.0);
    let frame = insp.frame(DT, &at(HD, OFF, false), false);
    let canvas = frame
        .surface(CANVAS_SLOT)
        .map(graph_view::seat_of)
        .expect("the canvas is seated");
    let seat = frame
        .surface(SAMPLE_SLOT)
        .map(graph_view::seat_of)
        .expect("the sample is seated");
    let slider = frame.rect(ZOOM).expect("the zoom slider is laid out");
    let graph = insp.canvas.zoom();
    insp.frame(DT, &wheel_at(centre(canvas), 1.0), false);
    assert_ne!(insp.canvas.zoom(), graph, "the canvas took its own notch");
    assert_eq!(insp.sample.zoom, 1.0, "…and the sample none");
    insp.frame(DT, &wheel_at(centre(slider), 1.0), false);
    assert_eq!(insp.sample.zoom, 1.0, "a notch beside the seat");

    // The walker's own sample for the seat with a notch on it, on a frame a menu owned.
    let mut owned = insp.frame(DT, &at(HD, OFF, false), false);
    owned.pointer = Some(SurfacePointer {
        id: SAMPLE_SLOT.into(),
        root: false,
        cursor: centre(seat),
        local: centre(seat) - seat.pos,
        delta: Vec2::ZERO,
        left: false,
        right: false,
        pressed: false,
        wheel: 1.0,
        captured: false,
        rect: seat,
    });
    owned.popup_hit = true;
    insp.zoom_sample(&owned);
    assert_eq!(insp.sample.zoom, 1.0, "a frame a menu owned is nobody's");
    owned.popup_hit = false;
    insp.zoom_sample(&owned);
    assert_eq!(
        insp.sample.zoom,
        1.0 + insp.zoom.step,
        "the same notch, no menu: it zooms"
    );
}

/// THE GRAPH, READ-ONLY, through the canvas's real pointer path: the walker hands the
/// canvas its sample over its seat, the cards are the nodes in evaluation order plus the
/// Output card (framed on open), a press on a node card PICKS it — `key · kind` and its
/// parameters read out, its card wears the selection — and nothing moves: a drag leaves
/// every card where it was. The Output card reads its wiring; empty canvas lets go.
#[test]
fn a_press_on_the_canvas_picks_a_card_and_reads_it_out() {
    load_strings();
    let package = scratch("pick");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    let mut insp = open(GRANITE, &package.0, &package.0);
    assert_eq!(
        text(&insp, NODE_LABEL),
        NODE_HINT,
        "nothing is picked on open"
    );
    assert!(
        insp.picked_selected().iter().all(|s| !s),
        "no card reads as picked"
    );

    insp.frame(DT, &at(HD, OFF, false), false);
    let keys = insp.canvas_keys();
    let graph = insp.facts().graph().expect("the graph loaded").clone();
    assert_eq!(
        keys.len(),
        graph.nodes.len() + 1,
        "a card per node, then Output"
    );
    assert_eq!(keys.last().map(String::as_str), Some(OUTPUT_CARD));
    let cards = insp.canvas_cards().to_vec();
    assert_eq!(cards.len(), keys.len(), "every card laid out");

    // The first node card the seat shows whole.
    let seat = insp
        .frame(DT, &at(HD, OFF, false), false)
        .surface(CANVAS_SLOT)
        .map(graph_view::seat_of)
        .expect("the canvas is seated");
    let inside = |r: &Rect| contains(seat, r.pos) && contains(seat, r.pos + r.size);
    let i = (0..keys.len() - 1)
        .find(|&i| inside(&cards[i]))
        .expect("a node card is framed on open");
    let at_card = centre(cards[i]);

    let frame = insp.frame(DT, &at(HD, at_card, true), false);
    assert!(
        frame.surface_pointer(CANVAS_SLOT).is_some(),
        "the walker hands the canvas its pointer"
    );
    let node = graph
        .nodes
        .iter()
        .find(|n| n.key == keys[i])
        .expect("the card is a node");
    let kind = strings::resolve(&format!("$mtl_kind_{}", node.kind.id())).into_owned();
    assert!(!kind.starts_with('$'), "the kind's NAME, resolved: {kind}");
    assert_eq!(
        text(&insp, NODE_LABEL),
        format!("{} · {kind}", node.key),
        "key · Kind"
    );
    assert!(
        !text(&insp, NODE_PARAMS).is_empty(),
        "its parameters read out"
    );
    // Its inputs: every port by its name, with the key wired there (a dash where none is).
    let dash = strings::resolve(NONE).into_owned();
    let want: Vec<String> = node
        .kind
        .ports()
        .iter()
        .enumerate()
        .map(|(p, port)| {
            let src = node.inputs.get(p).and_then(Option::as_deref);
            format!("{} {}", port.name, src.unwrap_or(&dash))
        })
        .collect();
    let inputs = if want.is_empty() {
        NONE.to_string()
    } else {
        want.join(" · ")
    };
    assert_eq!(text(&insp, NODE_INPUTS), inputs, "its inputs read out");
    assert!(
        insp.picked_selected()[i],
        "the picked card wears the selection"
    );

    // Read-only: a drag moves nothing.
    let before = insp.canvas_cards().to_vec();
    insp.frame(DT, &at(HD, at_card + Vec2::new(40.0, 25.0), false), false);
    insp.frame(DT, &at(HD, at_card + Vec2::new(40.0, 25.0), false), false);
    let mut held = at(HD, at_card + Vec2::new(60.0, 30.0), false);
    held.down = true;
    insp.frame(DT, &held, false);
    insp.frame(DT, &at(HD, OFF, false), false);
    assert_eq!(insp.canvas_cards(), &before[..], "no card moved");

    // The Output card — framed on open (it is the legible fit's focus) — reads its wiring.
    let out = insp.canvas_cards()[keys.len() - 1];
    assert!(inside(&out), "the Output card is in view on open");
    insp.frame(DT, &at(HD, centre(out), true), false);
    assert_eq!(
        text(&insp, NODE_LABEL),
        strings::resolve("$mtl_output"),
        "the Output card reads as itself"
    );
    assert!(
        text(&insp, NODE_INPUTS).contains("base "),
        "its port wiring reads as its inputs: {}",
        text(&insp, NODE_INPUTS)
    );
    assert!(
        text(&insp, NODE_PARAMS).contains("relief "),
        "and the rest of its stage as its parameters"
    );
    let picked = insp.picked_selected();
    assert!(
        picked[keys.len() - 1] && picked[..keys.len() - 1].iter().all(|s| !s),
        "the Output card wears the selection, no node card does"
    );
    insp.frame(DT, &at(HD, OFF, false), false);

    // Empty canvas lets go — a point of the seat no card covers.
    let cards = insp.canvas_cards().to_vec();
    let empty = (0..40)
        .flat_map(|y| (0..40).map(move |x| (x, y)))
        .map(|(x, y)| seat.pos + Vec2::new(seat.size.x * x as f32, seat.size.y * y as f32) / 40.0)
        .find(|p| !cards.iter().any(|c| contains(*c, *p)) && contains(seat, *p))
        .expect("the seat shows some empty canvas");
    insp.frame(DT, &at(HD, empty, true), false);
    assert_eq!(text(&insp, NODE_LABEL), NODE_HINT, "empty canvas lets go");
    assert_eq!(text(&insp, NODE_INPUTS), NONE);
    assert_eq!(text(&insp, NODE_PARAMS), "");
}

/// The graph is framed LEGIBLY on open, at the authored floor at worst (never smaller), and
/// the canvas draws its cards inside a scissor on its seat.
#[test]
fn the_graph_opens_framed_and_clipped_to_its_seat() {
    let package = scratch("frame");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    let mut insp = open(GRANITE, &package.0, &package.0);
    assert!((insp.fit_floor() - 0.75).abs() < 1e-6, "the authored floor");
    insp.frame(DT, &at(HD, OFF, false), false);
    let card = insp.canvas_cards()[0];
    assert!(
        card.size.x >= CanvasMetrics::default().card.x * insp.fit_floor() - 0.5,
        "framed at the floor at worst: {card:?}"
    );
    // The canvas draws LAST, over the walker's commands (the node list clips its own rows
    // in among those): its scissor is the seat, and the HUD ends released.
    let seat = insp
        .frame(DT, &at(HD, OFF, false), false)
        .surface(CANVAS_SLOT)
        .map(|s| [s.x, s.y, s.w, s.h])
        .expect("the canvas is seated");
    let clips: Vec<&HudCommand> = insp
        .hud()
        .iter()
        .filter(|c| matches!(c, HudCommand::Clip { .. }))
        .collect();
    let n = clips.len();
    assert!(
        n >= 2,
        "a scissor around the canvas, then released: {clips:?}"
    );
    assert!(
        matches!(clips[n - 2], HudCommand::Clip { rect: Some(r) } if *r == seat),
        "the canvas's scissor is its seat: {clips:?}"
    );
    assert!(matches!(clips[n - 1], HudCommand::Clip { rect: None }));
}

/// A GLOW opens WHOLE: the first curated glow (`glow_materials.json`, the four-node shape
/// every emissive slot ships) projects to its five cards — the nodes in evaluation order,
/// then the Output — and on open every one of them sits wholly inside the canvas's seat at
/// the legible floor, at 1080p and at 900p (the node list beside the canvas leaves it room
/// for two columns, and the centre stage keeps its authored height on both).
#[test]
fn a_glow_opens_with_all_five_cards_in_view() {
    let text = std::fs::read_to_string(format!("{DATA}/glow_materials.json"))
        .expect("the curated glows read");
    let doc: serde_json::Value = serde_json::from_str(&text).expect("the glows parse");
    let glow = doc["graphs"][0].to_string();
    let package = scratch("glow_whole");
    write_bundle(&package.0, 248, &glow, 8);
    for screen in [HD, Vec2::new(1600.0, 900.0)] {
        let mut insp = open(248, &package.0, &package.0);
        let frame = insp.frame(DT, &at(screen, OFF, false), false);
        assert_eq!(
            insp.canvas_keys(),
            ["cells", "veins", "body", "glow", OUTPUT_CARD],
            "four nodes in evaluation order, then the Output"
        );
        let seat = frame
            .surface(CANVAS_SLOT)
            .map(graph_view::seat_of)
            .expect("the canvas is seated");
        for (key, card) in insp.canvas_keys().iter().zip(insp.canvas_cards()) {
            assert!(
                contains(seat, card.pos) && contains(seat, card.pos + card.size),
                "`{key}` is in view on open at {screen:?}: {card:?} in {seat:?}"
            );
        }
    }
}

/// THE SLAB FITS EVERY SCREEN THE SHELL OFFERS (`flicker_shell::RESOLUTIONS`, its 1280×720
/// floor included) and the two common laptop panels besides: at each, the slab lies inside
/// the screen — its title on it — the PACKAGE header and the DEFINITION footer keep their
/// authored rows, the centre stage gives way only on a screen too short for it (at 900p and
/// up it keeps its authored height: the 1080p look), and Close is on screen inside the slab
/// and REACHABLE — a press at its rect closes with the host's answer.
#[test]
fn the_slab_fits_every_shell_resolution_and_close_stays_reachable() {
    load_strings();
    let raw = raw_tree();
    let mut all = Vec::new();
    raw_nodes(&raw, &mut all);
    let authored = |id: &str| -> f32 {
        all.iter()
            .find(|n| n.get("id").and_then(|i| i.as_str()) == Some(id))
            .and_then(|n| n.get("size"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| panic!("`{id}` authors a size")) as f32
    };
    let (header, footer, stage) = (
        authored("insp_pkg"),
        authored("insp_def"),
        authored("insp_centre"),
    );
    let package = scratch("fits");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    let screens: Vec<Vec2> = flicker_shell::RESOLUTIONS
        .iter()
        .map(|r| Vec2::new(r.w as f32, r.h as f32))
        .chain([Vec2::new(1366.0, 768.0), Vec2::new(1280.0, 800.0)])
        .collect();
    assert!(
        screens.iter().any(|s| s.y <= 720.0),
        "the shell's lowest rung is among them"
    );
    for screen in screens {
        for slot in [GRANITE, FREE] {
            let mut insp = open(slot, &package.0, &package.0);
            let frame = insp.frame(DT, &at(screen, OFF, false), false);
            let rect = |id: &str| {
                frame
                    .rect(id)
                    .unwrap_or_else(|| panic!("`{id}` is laid out at {screen:?}"))
            };
            let on_screen = |r: Rect| {
                r.pos.x >= 0.0
                    && r.pos.y >= 0.0
                    && r.pos.x + r.size.x <= screen.x
                    && r.pos.y + r.size.y <= screen.y
            };
            let popup = rect("popup");
            assert!(on_screen(popup), "the slab fits {screen:?}: {popup:?}");
            let close = rect("modal_close");
            assert!(
                on_screen(close)
                    && close.size.y > 20.0
                    && contains(popup, close.pos)
                    && contains(popup, close.pos + close.size),
                "Close is on screen inside the slab at {screen:?}: {close:?} in {popup:?}"
            );
            // The zoom slider keeps its row under the sample at every screen — the sample above
            // it is what gives way, never the control.
            let zoom = rect(ZOOM);
            assert!(
                on_screen(zoom)
                    && zoom.size.y >= 28.0
                    && contains(popup, zoom.pos)
                    && contains(popup, zoom.pos + zoom.size),
                "the zoom slider is on screen inside the slab at {screen:?}: {zoom:?}"
            );
            assert_eq!(rect("insp_pkg").size.y, header, "the header keeps its row");
            assert_eq!(rect("insp_def").size.y, footer, "the footer keeps its row");
            let given = rect("insp_centre").size.y;
            if screen.y >= 900.0 {
                assert_eq!(given, stage, "the authored centre stage at {screen:?}");
            } else {
                assert!(
                    given < stage && given >= 200.0,
                    "the centre stage gives way at {screen:?}: {given}"
                );
            }
            // Reachable: a press at Close's rect closes, with the host's answer.
            let pressed = insp.frame(DT, &at(screen, centre(close), true), false);
            assert!(
                pressed.results.is_on(MODAL_CANCEL),
                "Close answers a press at {screen:?}"
            );
            assert!(matches!(
                insp.answer(&pressed.results),
                Transition::CloseModal { .. }
            ));
        }
    }
}

/// A bundle's maps decode on the WORKER at the authored size and land for `render`.
#[test]
fn a_bundles_maps_decode_on_the_worker_at_the_authored_size() {
    let package = scratch("decode");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    let mut insp = open(GRANITE, &package.0, &package.0);
    assert!(insp.decoding(), "a bundle starts a decode");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !insp.take_decoded() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let landed = insp.pending.as_ref().expect("the maps land");
    assert_eq!(landed.slot, GRANITE);
    assert_eq!(landed.maps.px(), 512, "the tree's authored `params.map_px`");
}

/// THE NODE LIST: one row per card, in the canvas's order (every node in evaluation order,
/// then the Output), each labelled with its card's heading — and the PAD reads a node
/// through it: from the seeded focus the d-pad walks onto the rows, Confirm fires the row's
/// action, and the answer picks its card exactly as a click on the card does (the read-out
/// shows it, the card wears the selection). A pointer press on a row does the same. Close
/// stays the last stop of the ring, after every row.
#[test]
fn the_node_list_reads_a_node_by_pad_and_by_pointer() {
    load_strings();
    let package = scratch("list");
    write_bundle(&package.0, GRANITE, &v2_doc(), 8);
    let mut insp = open(GRANITE, &package.0, &package.0);
    let keys = insp.canvas_keys();
    let graph = insp.facts().graph().expect("the graph loaded").clone();
    assert_eq!(keys.len(), graph.nodes.len() + 1);
    // Each row's label is its card's heading, published raw on the row's `label_bind`.
    for (i, key) in keys.iter().enumerate() {
        let label = text(&insp, &format!("insp_nl{i}_label")).to_string();
        match graph.nodes.iter().find(|n| &n.key == key) {
            Some(node) => {
                let kind = strings::resolve(&format!("$mtl_kind_{}", node.kind.id())).into_owned();
                assert_eq!(label, format!("{key} · {kind}"), "row {i}");
            }
            None => {
                assert_eq!(key, OUTPUT_CARD, "the last row is the Output");
                assert_eq!(label, strings::resolve("$mtl_output"), "row {i}");
            }
        }
    }

    // The pad: seeded on the toggle, Down steps onto the zoom slider under it, Down again
    // onto the first row (the slider's own axis is Left / Right), Confirm picks it.
    let frame = insp.frame(DT, &at(HD, OFF, false), false);
    assert!(
        frame.rect("insp_nl_0").is_some(),
        "the first row is laid out"
    );
    assert_eq!(insp.ui_state().focused(), Some("insp_body"), "seeded focus");
    let raw = InputState::new();
    let mut rc = RouteCtx::new();
    let (tree, model, intents) = (
        insp.tree().clone(),
        insp.model().clone(),
        insp.intents().clone(),
    );
    let fired = {
        let mut h = WalkerHandler::hud(insp.ui_state(), false)
            .with_nav(&tree, &model)
            .with_rects(&frame.rects)
            .with_intents(&intents);
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        h.handle(&press(ActionSignal::Confirm, &raw), &mut rc);
        h.take_fired()
    };
    assert_eq!(fired, vec![row_action(0)], "Confirm on the first row");
    let mut results = ValueMap::new();
    results.set(row_action(0), true);
    assert!(
        matches!(insp.answer(&results), Transition::None),
        "a pick does not close"
    );
    let first = graph
        .nodes
        .iter()
        .find(|n| n.key == keys[0])
        .expect("the first card is a node");
    let kind = strings::resolve(&format!("$mtl_kind_{}", first.kind.id())).into_owned();
    assert_eq!(text(&insp, NODE_LABEL), format!("{} · {kind}", first.key));
    let picked = insp.picked_selected();
    assert!(
        picked[0] && picked[1..].iter().all(|s| !s),
        "its card wears the selection"
    );

    // Close stays last: from the last row, the ring steps on to Close.
    let last = keys.len() - 1;
    insp.ui_state().request_focus(format!("insp_nl_{last}"));
    {
        let mut h = WalkerHandler::hud(insp.ui_state(), false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
    }
    assert_eq!(insp.ui_state().focused(), Some("modal_close"));

    // The pointer: a press on the Output's row picks the Output card — the wheel over the
    // list first brings that row into view (the list's offset rides `insp_node_scroll`, which
    // the host folds back into the Model every frame).
    let frame = insp.frame(DT, &at(HD, OFF, false), false);
    let list = frame.rect("insp_node_list").expect("the list is laid out");
    let row_id = format!("insp_nl_{last}");
    let mut row = frame.rect(&row_id).expect("the Output's row is laid out");
    let shown = |r: Rect| contains(list, r.pos) && contains(list, r.pos + r.size);
    for _ in 0..40 {
        if shown(row) {
            break;
        }
        let mut wheel = at(HD, centre(list), false);
        wheel.wheel = -1.0;
        insp.frame(DT, &wheel, false);
        row = insp
            .frame(DT, &at(HD, OFF, false), false)
            .rect(&row_id)
            .expect("the row stays laid out");
        assert!(
            insp.model().number(NODE_SCROLL).is_some_and(|o| o > 0.0),
            "the list's offset is folded back into the Model"
        );
    }
    assert!(
        shown(row),
        "the wheel brings the last row into view: {row:?} in {list:?}"
    );
    let frame = insp.frame(DT, &at(HD, centre(row), true), false);
    assert!(
        frame.results.is_on(&row_action(last)),
        "the click fires the row"
    );
    insp.answer(&frame.results);
    assert_eq!(text(&insp, NODE_LABEL), strings::resolve("$mtl_output"));
    assert!(
        insp.picked_selected()[last],
        "the Output card wears the selection"
    );
}

/// A BUNDLE SHORT OF ITS MAPS SAYS SO: a recipe with no map at all reads a dash for its map
/// set (never an empty string) and the INCOMPLETE note — never "promoted" — and so does one
/// short of a single map (its set lists what it holds). Neither starts a decode: a sample
/// is drawn in a whole material or not at all.
#[test]
fn a_bundle_short_of_its_maps_says_so() {
    load_strings();
    let none = scratch("short_none");
    // A recipe and NO maps.
    let bare = scratch("short_bare");
    let dir = flicker_content::bundle_dir(&bare.0, GRANITE);
    std::fs::create_dir_all(&dir).unwrap();
    let seg = flicker_materials::slot_dir(GRANITE);
    flicker_content::package::write_text(&dir.join(format!("{seg}.texture.json")), &v2_doc())
        .expect("the recipe writes");
    let insp = open(GRANITE, &bare.0, &none.0);
    assert_eq!(
        insp.facts().held,
        Held::Package,
        "a recipe alone is a bundle"
    );
    assert!(insp.facts().maps.is_empty());
    assert_eq!(
        text(&insp, "insp_pkg_maps"),
        NONE,
        "no map set reads a dash"
    );
    assert_eq!(text(&insp, "insp_pkg_note"), "$insp_note_incomplete");
    assert!(!insp.decoding(), "nothing whole to decode");
    assert!(!strings::resolve("$insp_note_incomplete").starts_with('$'));

    // One map short.
    let short = scratch("short_one");
    write_bundle(&short.0, GRANITE, &v2_doc(), 8);
    let dir = flicker_content::bundle_dir(&short.0, GRANITE);
    std::fs::remove_file(dir.join(format!("{seg}_Emit.png"))).expect("the Emit map goes");
    let insp = open(GRANITE, &short.0, &none.0);
    assert_eq!(insp.facts().maps.len(), MapKind::ALL.len() - 1);
    assert_eq!(text(&insp, "insp_pkg_note"), "$insp_note_incomplete");
    assert!(!text(&insp, "insp_pkg_maps").contains("Emit"));
    assert!(!insp.decoding());

    // Whole again: promoted, decoding.
    let whole = scratch("short_whole");
    write_bundle(&whole.0, GRANITE, &v2_doc(), 8);
    let insp = open(GRANITE, &whole.0, &none.0);
    assert_eq!(text(&insp, "insp_pkg_note"), "$insp_note_package");
    assert!(insp.decoding());
}

/// CLOSING MID-DECODE NEVER HOLDS THE FRAME (the 48 ms / 181 ms stall the per-open pool's
/// join cost at 2K / 4K): the inspector borrows a long-lived pool and owns no thread, so
/// stopping and dropping it with a big decode IN FLIGHT returns in under a millisecond — and
/// the decode, cancelled between maps, frees the worker after at most the map in hand.
#[test]
fn closing_mid_decode_never_holds_the_frame() {
    let package = scratch("midclose");
    write_bundle(&package.0, GRANITE, &v2_doc(), 2048);
    // What one whole decode costs on this machine, at the authored size.
    let bundle = bundle_at(&package.0, GRANITE).expect("the bundle is defined");
    let t = Instant::now();
    flicker_materialview::decode_bundle_maps(&bundle, 512).expect("the bundle decodes whole");
    let whole = t.elapsed();
    assert!(
        whole > Duration::from_millis(20),
        "a decode long enough to close in the middle of: {whole:?}"
    );

    let pool = WorkerPool::new(1);
    let mut insp = MaterialInspector::new(gather(GRANITE, &package.0, &package.0), &pool);
    assert!(insp.decoding(), "the decode is queued on the borrowed pool");
    // Well into the bundle, with maps still to go.
    std::thread::sleep(whole / 3);
    let t = Instant::now();
    insp.stop();
    drop(insp);
    let closed = t.elapsed();
    assert!(
        closed < Duration::from_millis(1),
        "closing mid-decode held the frame for {closed:?} (a whole decode is {whole:?})"
    );

    // The worker is free again after at most the map in hand — never the rest of the bundle.
    let (tx, rx) = mpsc::channel();
    let t = Instant::now();
    pool.submit(move || {
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(30))
        .expect("the worker frees");
    let freed = t.elapsed();
    eprintln!("closing mid-decode: closed in {closed:?}, worker freed in {freed:?}, a whole decode {whole:?}");
    assert!(
        freed < whole / 2,
        "the cancelled decode ran on: the worker freed after {freed:?}, a whole decode is \
         {whole:?}"
    );
}

/// DEVELOPMENT-TIER GATES: tests that read this module's own source.
mod gates {
    use super::*;

    /// The shipped half of `inspector.rs` (its `#[cfg(test)]` tail cut off).
    fn shipped_source() -> String {
        let src = include_str!("../inspector.rs").replace("\r\n", "\n");
        src.split_once("#[cfg(test)]\nmod tests;")
            .map(|(before, _)| before.to_string())
            .unwrap_or(src)
    }

    /// NOTHING THE INSPECTOR HOLDS SURVIVES ITS EXIT (rule 728E682F): it owns no render target,
    /// mesh or composite of its own — the sample is `flicker-materialview`'s, the target its
    /// `GlobeView` — and its `exit` gives back the sample's target, a bundle's uploaded maps,
    /// the flat textures (the swatch among them), the bodies' meshes, stops the decode and drops
    /// the canvas's gesture.
    #[test]
    fn nothing_the_inspector_holds_survives_its_exit() {
        let src = shipped_source();
        for needle in [
            "create_render_target",
            "free_render_target",
            "resize_render_target",
            "upload_textured_mesh",
            "free_textured_mesh",
            "draw_textured_mesh_pbr",
            "composite_panel",
            "fg.surface(",
            "CompositeTarget",
            "Theme::build",
            "load_texture(",
        ] {
            assert!(
                !src.contains(needle),
                "inspector.rs: `{needle}` belongs to flicker-materialview / the theme"
            );
        }
        let body_of = |signature: &str| {
            src.split_once(signature)
                .map(|(_, body)| body.split_once("\n    }\n").map_or(body, |(b, _)| b))
                .unwrap_or_else(|| panic!("the inspector implements `{signature}`"))
                .to_string()
        };
        let exit = body_of("fn exit(&mut self, renderer: &mut Renderer) {");
        for owed in [
            "self.stop()",
            "self.sample.release(renderer)",
            "maps.release(renderer)",
            "std::mem::take(&mut self.flats).release(renderer)",
            "bodies.release(renderer)",
        ] {
            assert!(exit.contains(owed), "exit owes `{owed}`");
        }
        let stop = body_of("pub(crate) fn stop(&mut self) {");
        for owed in ["self.cancel.store(true", "self.canvas.cancel()"] {
            assert!(stop.contains(owed), "stop owes `{owed}`");
        }
        // It owns no pool: a pool held here would be joined when the modal drops.
        assert!(
            !src.contains("WorkerPool::new") && !src.contains("pool: WorkerPool"),
            "the inspector borrows the catalog's pool and never owns one"
        );
    }

    /// The MODEL-CHANNEL strings gate for this module: no raw display copy published from Rust.
    #[test]
    fn no_raw_display_copy_is_published_into_the_model() {
        let flagged = strings::raw_model_publish_literals(&shipped_source());
        assert!(
            flagged.is_empty(),
            "inspector.rs publishes raw display copy: {flagged:?}"
        );
    }
}
