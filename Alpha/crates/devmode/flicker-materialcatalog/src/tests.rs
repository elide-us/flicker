//! The catalog's drift gates and its behaviour's contract.
//!
//! None of this needs a GPU: the authored tree, the sample stage, the Model, the pair
//! script, the styles, the per-slot samples, the walker's seats and the worker's decoded
//! maps are all reachable without a device. The gates pin the three authored artifacts
//! (scene tree · pair script · Rust vocabulary) to each other, so a rename in one is loud in
//! the others.

use flicker::render::{AttachmentFormat, Attachments, BloomPass, PassKind, Rate, Vec2};
use flicker::script::{HudCommand, Value};
use flicker::ui::{run_ui, strings, UiInput, UiState};
use flicker_materials::RenderClass;
use flicker_texture::MapKind;

use super::*;

/// The authored grid heads: a corner blank, one head per column, one per row — each with
/// explicit `col`/`row`, so the cells auto-flow around them.
const AUTHORED_HEADS: usize = 1 + GRID_COLS + SLOTS / GRID_COLS;

/// The shipped stringtable, so a walked frame carries resolved en-us text rather than raw
/// `$token`s.
fn load_strings() {
    let table = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    strings::load_str(&table, "en-us");
}

/// A catalog on the SHIPPED scene file — the same authored tree the runtime gets.
fn shipped() -> MaterialCatalog {
    MaterialCatalog::new(&shipped_def())
}

fn shipped_def() -> SceneDef {
    SceneDef::parse("materialcatalog", MC_SCENE)
        .expect("the shipped materialcatalog.scene.json parses")
}

fn tree_of(cat: &MaterialCatalog) -> UiNode {
    cat.authored_tree()
        .expect("the shipped scene file declares a tree")
}

fn flatten<'a>(node: &'a UiNode, out: &mut Vec<&'a UiNode>) {
    out.push(node);
    for kid in &node.children {
        flatten(kid, out);
    }
}

fn grid_of(tree: &UiNode) -> &UiNode {
    let mut all = Vec::new();
    flatten(tree, &mut all);
    all.into_iter()
        .find(|n| n.id == "mc_grid")
        .expect("the catalog grid is authored")
}

/// A headless 1920×1080 frame, pointer parked off-screen.
fn snap_1080p() -> UiInput {
    UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        right_down: false,
        screen: Vec2::new(1920.0, 1080.0),
        wheel: 0.0,
        exclusive: false,
        motion: Default::default(),
    }
}

/// A scratch package root, unique per process (pid) and per call (atomic counter) so two
/// concurrent `cargo test` processes never share a fixed dir.
fn package_scratch(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!(
        "flicker_mc_package_{name}_{}_{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The seven maps' colours, in [`MapKind::ALL`] order — each its own, so a map read into
/// the wrong slot shows.
const MAP_COLOURS: [[u8; 3]; 7] = [
    [200, 90, 30],
    [128, 128, 255],
    [60, 60, 60],
    [250, 250, 250],
    [180, 180, 180],
    [10, 20, 30],
    [255, 140, 0],
];

/// A real bundle for an assigned slot that may hold its row (the slot law) in a scratch
/// package root: seven 8×8 solid PNGs ([`MAP_COLOURS`]) and the recipe, in the one layout
/// `bundle_dir` spells, found by the P3 predicate. Returns the root (remove it when done),
/// the slot and the bundle.
fn scratch_bundle(name: &str) -> (std::path::PathBuf, MaterialId, MaterialBundle) {
    let root = package_scratch(name);
    let slot = shipped()
        .materials
        .iter()
        .find(|m| flicker_materials::slot_accepts(m.id, m.render_class))
        .map(|m| m.id)
        .expect("a material in the index whose slot may hold it");
    let dir = flicker_content::bundle_dir(&root, slot);
    std::fs::create_dir_all(&dir).unwrap();
    let seg = flicker_materials::slot_dir(slot);
    for (kind, rgb) in MapKind::ALL.into_iter().zip(MAP_COLOURS) {
        image::RgbImage::from_pixel(8, 8, image::Rgb(rgb))
            .save(dir.join(format!("{seg}_{}.png", kind.role())))
            .expect("a map encodes");
    }
    flicker_content::package::write_text(&dir.join(format!("{seg}.texture.json")), "{}")
        .expect("the recipe writes");
    let bundle = flicker_content::bundle_at(&root, slot).expect("the P3 predicate sees it");
    (root, slot, bundle)
}

/// The one `bloom` pass a compiled stage runs.
fn bloom_of(def: &StageDef, source: &str) -> BloomPass {
    let blooms: Vec<BloomPass> = def
        .recipe()
        .iter()
        .filter_map(|p| match &p.kind {
            PassKind::Bloom(b) => Some(*b),
            _ => None,
        })
        .collect();
    assert_eq!(
        blooms.len(),
        1,
        "stages.{source} runs one bloom: {blooms:?}"
    );
    blooms[0]
}

// ── drift gates ───────────────────────────────────────────────────────────────

/// The scene file itself parses — the manifest's copy and this crate's are the same
/// bytes, so this is the authoring gate.
#[test]
fn the_shipped_scene_file_parses_with_a_tree_and_styles() {
    let def = SceneDef::parse("materialcatalog", MC_SCENE).expect("scene file parses");
    assert_eq!(
        def.behaviour, "materialcatalog",
        "the file names its own behaviour"
    );
    assert!(def.tree.is_some(), "the scene file declares a tree");
    assert!(
        def.styles.is_some(),
        "the scene file carries this scene's style blocks"
    );
}

/// The VOCABULARY gate. An unknown kind renders NOTHING, so a typo would be an invisible
/// hole nobody noticed until they opened the catalog.
#[test]
fn the_hud_names_only_known_kinds() {
    let tree = tree_of(&shipped());
    let unknown = flicker::ui::unknown_kinds(&tree);
    assert!(
        unknown.is_empty(),
        "the catalog names unknown kinds: {unknown:?}"
    );
}

/// The STRINGS gate (tree channel): every display literal is a `$token`, so no English
/// ships unlocalisable. The grid heads are digits, which are not copy.
#[test]
fn the_hud_ships_no_raw_display_literals() {
    let tree = tree_of(&shipped());
    let raw = flicker::ui::raw_display_literals(&tree);
    assert!(
        raw.is_empty(),
        "the catalog ships raw display literals: {raw:?}"
    );
}

/// Every `$token` the catalog publishes or the tree names must EXIST in the stringtable —
/// a token that resolves to nothing renders blank and looks like a layout bug.
#[test]
fn every_token_the_catalog_draws_resolves() {
    load_strings();
    let cat = shipped();
    let tree = tree_of(&cat);
    let frame = run_ui(
        &tree,
        &cat.model(),
        &cat.ui_styles,
        &snap_1080p(),
        &mut UiState::new(),
    );
    let unresolved: Vec<String> = frame
        .commands
        .iter()
        .filter_map(|c| match c {
            HudCommand::Text { text, .. } if text.starts_with('$') => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        unresolved.is_empty(),
        "tokens with no stringtable entry: {unresolved:?}"
    );
}

/// Every style path the tree authors resolves in the merged styles: a block for a
/// `style`, a colour for a `backdrop` or a `color`. A path that resolves to nothing draws
/// compiled defaults SILENTLY.
#[test]
fn every_authored_style_path_resolves() {
    let cat = shipped();
    let tree = tree_of(&cat);
    let mut all = Vec::new();
    flatten(&tree, &mut all);
    let lookup = |path: &str| -> Option<&serde_json::Value> {
        path.split('.').try_fold(&cat.ui_styles, |v, k| v.get(k))
    };
    let mut checked = 0;
    for n in &all {
        for key in ["style", "runes_style"] {
            if let Some(Value::Text(path)) = n.props.get(key) {
                assert!(
                    lookup(path).is_some_and(|v| v.is_object()),
                    "{}: `{key}` path {path} resolves to no block",
                    n.component
                );
                checked += 1;
            }
        }
        for key in ["backdrop", "color"] {
            if let Some(Value::Text(path)) = n.props.get(key) {
                assert!(
                    lookup(path).is_some_and(|v| v.is_array() || v.is_string()),
                    "{}: `{key}` path {path} resolves to no colour",
                    n.component
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked > 300,
        "the sweep saw the authored paths ({checked})"
    );
}

// ── the authored grid ─────────────────────────────────────────────────────────

/// The grid is AUTHORED whole: the heads with explicit placement, then every slot of the
/// byte once, in id order — whichever slots the index assigns — each cell a PRESSABLE
/// `panel` (the standard full-rect control: it flows its children like a cell and fires its
/// `action` on a click or a pad Confirm) firing that slot's `mc_open_<id>`, a member of the
/// `mc_panel` pane at ordinal id + 1, holding a `surface` on the scene's sample stage, a
/// POSTER (authored on the node: the walker reads liveness off the node, never the stage),
/// whose panel is the well, and a text bound to that slot's name. Nothing is composed in
/// Rust.
#[test]
fn the_grid_authors_every_slot_once_in_order() {
    let cat = shipped();
    let tree = tree_of(&cat);
    let grid = grid_of(&tree);
    assert_eq!(grid.component, "grid");
    let tracks = |key: &str| match grid.props.get(key) {
        Some(Value::Text(spec)) => spec.split_whitespace().count(),
        other => panic!("the grid authors no `{key}` track spec: {other:?}"),
    };
    assert_eq!(tracks("cols"), GRID_COLS + 1, "a head column and sixteen");
    assert_eq!(
        tracks("rows"),
        SLOTS / GRID_COLS + 1,
        "a head row and sixteen"
    );

    assert_eq!(grid.children.len(), AUTHORED_HEADS + SLOTS);
    for head in &grid.children[..AUTHORED_HEADS] {
        assert_eq!(head.component, "text", "a head is a text");
        assert!(
            head.props.contains_key("col") && head.props.contains_key("row"),
            "a head is placed explicitly so the cells flow around it"
        );
    }
    let text = |s: &str| Some(Value::Text(s.into()));
    for (k, cell) in grid.children[AUTHORED_HEADS..].iter().enumerate() {
        let slot = k as MaterialId;
        assert_eq!(cell.component, "panel", "slot {slot}: a pressable cell");
        assert_eq!(cell.id, cell_id(slot));
        assert_eq!(
            cell.action.as_deref(),
            Some(open_action(slot).as_str()),
            "slot {slot}: the box opens its own inspector"
        );
        assert_eq!(cell.tab_group, "mc_panel", "slot {slot}: in the grid pane");
        assert_eq!(
            cell.nav_ordinal,
            u32::from(slot) + 1,
            "slot {slot}: the pad walks the byte in id order"
        );
        assert_eq!(
            cell.props.get("style").cloned(),
            text("catalog.cell"),
            "slot {slot}: the focus rim is the scene's own block"
        );
        assert!(
            !cell.props.contains_key("col"),
            "a slot cell auto-flows — its position IS its id"
        );
        assert_eq!(cell.children.len(), 2, "slot {slot}: a box and a name row");
        let (surface, label) = (&cell.children[0], &cell.children[1]);
        assert_eq!(surface.component, "surface", "slot {slot}");
        assert_eq!(surface.id, box_id(slot));
        assert_eq!(
            surface.props.get("source").cloned(),
            text(SAMPLE_STAGE_SOURCE),
            "slot {slot}: the box is drawn under the scene's sample stage"
        );
        assert_eq!(
            surface.props.get("rate").cloned(),
            text("poster"),
            "slot {slot}: a poster, authored on the node"
        );
        assert_eq!(
            surface.props.get("style").cloned(),
            text("catalog.well"),
            "slot {slot}: the box's panel is the well a free slot shows alone"
        );
        assert_eq!(label.component, "text", "slot {slot}");
        assert_eq!(
            label.props.get("text_bind"),
            Some(&Value::Text(name_key(slot))),
            "slot {slot}: the name row binds the slot's name"
        );
    }
}

/// The texture PLAN is gone: no authored `sprite` indexes a texture table any more — the
/// boxes are surfaces, and the HUD's table is the theme's. A `tex` left on a sprite would
/// draw whatever the theme's table holds at that index: silently the wrong picture.
#[test]
fn no_sprite_indexes_a_texture_table() {
    let tree = tree_of(&shipped());
    let mut all = Vec::new();
    flatten(&tree, &mut all);
    let textured: Vec<&str> = all
        .iter()
        .filter(|n| n.component == "sprite" && n.props.contains_key("tex"))
        .map(|n| n.id.as_str())
        .collect();
    assert!(
        textured.is_empty(),
        "sprites still index a texture table: {textured:?}"
    );
    let boxes = all
        .iter()
        .filter(|n| n.component == "surface" && n.id.starts_with("mc_box_"))
        .count();
    assert_eq!(boxes, SLOTS, "every box is a surface");
}

// ── the sample stage + the spheres ────────────────────────────────────────────

/// The scene's own sample stage compiles CLEAN through the one stage compiler: HDR
/// (an rgba16f `hdr` attachment, with depth), `bloom` after the scene pass it spreads,
/// exactly one `tonemap_grade` resolving LAST, the `material` layer (the sphere) and nothing
/// the sample cannot draw, a framing, a transparent clear (the well is the backdrop), and a
/// finite emissive gain of at least one — the catalog compiled the same definition.
#[test]
fn the_sample_stage_is_hdr_blooms_and_tonemaps_last() {
    let cat = shipped();
    let (def, problems) = flicker::ui::compile_stage(&cat.ui_styles, SAMPLE_STAGE_SOURCE)
        .expect("the scene authors stages.mc_sample");
    assert!(
        problems.is_empty(),
        "stages.{SAMPLE_STAGE_SOURCE} has authoring problems: {problems:?}"
    );
    assert!(
        def.has_layer("material"),
        "the `material` layer IS the sphere"
    );
    assert!(
        def.layers_outside(flicker_materialview::SAMPLE_LAYERS)
            .is_empty(),
        "it authors nothing the sample cannot draw"
    );
    assert!(def.camera.is_some(), "the stage frames the sphere");
    assert_eq!(
        def.clear,
        Some([0.0; 4]),
        "a transparent clear, so the well is the backdrop"
    );
    assert_eq!(
        def.attachments.get(Attachments::HDR).map(|a| a.format),
        Some(AttachmentFormat::Rgba16f),
        "HDR"
    );
    assert!(
        def.attachments.get(Attachments::DEPTH).is_some(),
        "with depth"
    );

    let (order, cyclic) = def.pass_order();
    assert!(!cyclic, "the recipe orders itself");
    let kinds: Vec<&str> = order.iter().map(|&i| def.recipe()[i].kind.kind()).collect();
    let at = |kind: &str| {
        kinds
            .iter()
            .position(|k| *k == kind)
            .unwrap_or_else(|| panic!("no `{kind}` pass in {kinds:?}"))
    };
    assert!(
        at("scene") < at("bloom"),
        "bloom spreads what the scene wrote"
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == "tonemap_grade").count(),
        1,
        "exactly one tonemap: {kinds:?}"
    );
    assert_eq!(
        kinds.last(),
        Some(&"tonemap_grade"),
        "the tonemap resolves LAST: {kinds:?}"
    );

    assert!(
        def.emissive_gain.is_finite() && def.emissive_gain >= 1.0,
        "the spheres' glow has a real authored gain, found {}",
        def.emissive_gain
    );
    let compiled = cat.stage.as_ref().expect("the catalog compiled its stage");
    assert_eq!(compiled.emissive_gain, def.emissive_gain);
    assert!(compiled.has_layer("material"));
}

/// One sample per ASSIGNED slot, in id order, seated by its own box's id — the emissive
/// slots' glows included — and no sample for a slot the index leaves free: those boxes are
/// the authored well.
#[test]
fn every_assigned_slot_wears_one_sample_and_no_other_slot_does() {
    let cat = shipped();
    assert!(cat.materials.len() >= 2, "the index loaded");
    let sampled: Vec<MaterialId> = cat.samples.iter().map(|s| s.slot).collect();
    let assigned: Vec<MaterialId> = cat.materials.iter().map(|m| m.id).collect();
    assert_eq!(
        sampled, assigned,
        "one sample per assigned slot, in id order"
    );
    for s in &cat.samples {
        assert_eq!(
            s.node,
            box_id(s.slot),
            "slot {}: seated in its own box",
            s.slot
        );
    }
    for slot in 0..=MaterialId::MAX {
        assert_eq!(
            cat.samples.iter().any(|s| s.slot == slot),
            assigned.contains(&slot),
            "slot {slot}: a sample exactly when assigned"
        );
    }
}

/// A sphere with no bundle wears the flat PBR its index row reads as — through the crate's
/// REAL data loader: the shipped emissive-class row glows in its own colour, and a metal row
/// is metallic in its index colour with no glow.
#[test]
fn a_flat_sphere_wears_its_rows_finish_and_glow() {
    let cat = shipped();
    let flat_of = |what: &str, pick: &dyn Fn(&MaterialDef) -> bool| {
        let row = cat
            .materials
            .iter()
            .find(|&m| pick(m))
            .unwrap_or_else(|| panic!("the shipped index has no {what} row"));
        let s = cat
            .samples
            .iter()
            .find(|s| s.slot == row.id)
            .expect("an assigned row has a sample");
        assert_eq!(s.flat, FlatMaterial::of(row), "{}: wears its row", row.name);
        (row, s.flat)
    };

    let (glow, f) = flat_of("emissive-class", &|m| {
        m.render_class == Some(RenderClass::Emissive)
    });
    assert_eq!(
        f.emissive,
        Some(glow.color),
        "{} glows in its own colour",
        glow.name
    );

    let (metal, f) = flat_of("metal", &|m| m.category == "metal");
    assert_eq!(f.metalness, 1.0, "{} is metallic", metal.name);
    assert_eq!(f.color, metal.color, "the index colour is the albedo");
    assert_eq!(f.emissive, None, "{} does not glow", metal.name);
}

/// Walked at 1080p, every assigned slot's sample finds the seat its box reserved, by node
/// id — and one whose box reserved nothing is NAMED (returned, and warned the first time),
/// never left a silent empty box.
#[test]
fn every_sample_finds_its_box_and_a_missing_one_is_named() {
    let mut cat = shipped();
    let tree = tree_of(&cat);
    let frame = run_ui(
        &tree,
        &cat.model(),
        &cat.ui_styles,
        &snap_1080p(),
        &mut UiState::new(),
    );
    assert!(
        cat.seat_samples(&frame.surfaces).is_empty(),
        "every assigned slot's box reserved its seat"
    );
    let gone = cat.samples[0].slot;
    let short: Vec<SurfaceSlot> = frame
        .surfaces
        .iter()
        .filter(|s| s.id != box_id(gone))
        .cloned()
        .collect();
    assert_eq!(
        cat.seat_samples(&short),
        vec![gone],
        "a box that reserved nothing is named"
    );
}

// ── the model + the pair script ───────────────────────────────────────────────

/// Every index row publishes its name under its slot (data, never a token) — the emissive
/// slots' glows included: an ASSIGNED emissive slot shows its material's name, and the pair
/// script leaves it alone; a free slot below the block says nothing.
#[test]
fn the_pair_script_derives_the_emissive_block_labels() {
    let cat = shipped();
    assert!(
        cat.script.is_some(),
        "materialcatalog.lua loads (the pair script)"
    );
    let m = cat.model();
    for def in &cat.materials {
        assert_eq!(
            m.text(&name_key(def.id)),
            Some(def.name.as_str()),
            "slot {} publishes its name",
            def.id
        );
    }
    assert_eq!(m.number("mc_slot_count"), Some(SLOTS as f64));
    assert_eq!(m.number("mc_emissive_first"), Some(EMISSIVE_FIRST as f64));
    for slot in EMISSIVE_FIRST..=MaterialId::MAX {
        let row = cat
            .materials
            .iter()
            .find(|d| d.id == slot)
            .unwrap_or_else(|| panic!("emissive slot {slot} holds a row"));
        assert_eq!(
            m.text(&name_key(slot)),
            Some(row.name.as_str()),
            "assigned emissive slot {slot} shows its material's name, never the held label"
        );
    }
    let free = (1..EMISSIVE_FIRST)
        .find(|id| cat.materials.iter().all(|m| m.id != *id))
        .expect("a free slot below the block");
    assert!(
        m.get(&name_key(free)).is_none(),
        "free slot {free} publishes no name"
    );
}

/// THE MECHANISM, kept for the slots it is for: an emissive slot that holds NO row is
/// labelled with the stringtable's word for what it is held for — derived by the pair
/// script from the two published numbers alone — while an assigned one keeps its name and
/// a free slot below the block says nothing. Driven with the shipped model minus three of
/// the block's names, as a catalog whose index left those emissive slots open would publish.
#[test]
fn an_open_emissive_slot_is_labelled_held_and_an_assigned_one_is_not() {
    let cat = shipped();
    let open = [EMISSIVE_FIRST, EMISSIVE_FIRST + 3, MaterialId::MAX];
    let mut raw = ValueMap::default();
    for (key, value) in cat.hud_model().entries() {
        if !open.iter().any(|&slot| *key == name_key(slot)) {
            raw.set(key.clone(), value.clone());
        }
    }
    let script = ScriptHost::new(MC_SCRIPT, "materialcatalog.lua").expect("the pair script");
    script.set_model(&raw).expect("the model publishes");
    let derived = script.derive().expect("derive runs").unwrap_or_default();
    for slot in EMISSIVE_FIRST..=MaterialId::MAX {
        let label = derived.text(&name_key(slot));
        if open.contains(&slot) {
            assert_eq!(label, Some("$mc_emissive"), "open emissive slot {slot}");
        } else {
            assert_eq!(label, None, "assigned emissive slot {slot} keeps its name");
        }
    }
    let free = (1..EMISSIVE_FIRST)
        .find(|id| cat.materials.iter().all(|m| m.id != *id))
        .expect("a free slot below the block");
    assert_eq!(
        derived.get(&name_key(free)),
        None,
        "a free slot says nothing"
    );
}

/// The tally PARTITIONS the byte, every count pre-formatted as digits (the tree never does
/// arithmetic): assigned (every row — the eight glows in the emissive slots among them) +
/// emissive OPEN (emissive slots holding no row: none, now) + free = 256.
#[test]
fn the_tally_adds_up_to_the_byte() {
    let cat = shipped();
    let m = cat.model();
    let n = |key: &str| -> usize {
        m.text(key)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("{key} is published as digits: {:?}", m.get(key)))
    };
    assert_eq!(n("mc_assigned"), cat.materials.len());
    assert_eq!(n("mc_generated"), cat.generated.len());
    let open = (EMISSIVE_FIRST..=MaterialId::MAX)
        .filter(|&slot| cat.materials.iter().all(|d| d.id != slot))
        .count();
    assert_eq!(open, 0, "the shipped index fills every emissive slot");
    assert_eq!(n("mc_emissive"), open);
    assert_eq!(
        n("mc_assigned") + n("mc_free") + n("mc_emissive"),
        SLOTS,
        "assigned + free + emissive open is the whole byte"
    );
    // The emissive slots are exactly the ones the slot law holds for emissive rows.
    let held: Vec<MaterialId> = (0..=MaterialId::MAX)
        .filter(|&s| held_for_emissive(s))
        .collect();
    assert_eq!(held.len(), EMISSIVE_SLOTS);
    assert_eq!(held.first(), Some(&EMISSIVE_FIRST));
}

/// THE EIGHT GLOWS in the catalog: every emissive slot holds an emissive-class row wearing
/// the curated palette's colour in palette order (Lava, 62, is the one emissive row outside
/// the block), and its sphere's flat PBR — what the catalog draws with no bundle in the
/// package — EMITS that palette colour, at the stage's authored gain.
#[test]
fn the_emissive_slots_hold_the_curated_glows_and_their_spheres_burn() {
    let cat = shipped();
    let palette = flicker_materialview::GlowPalette::load(&flicker_content::roots().data());
    assert_eq!(
        palette.len(),
        EMISSIVE_SLOTS,
        "one curated colour per emissive slot"
    );
    let emissive: Vec<&MaterialDef> = cat
        .materials
        .iter()
        .filter(|m| m.render_class == Some(RenderClass::Emissive))
        .collect();
    assert_eq!(
        emissive.len(),
        EMISSIVE_SLOTS + 1,
        "the eight glows and Lava: {:?}",
        emissive.iter().map(|m| (m.id, &m.name)).collect::<Vec<_>>()
    );
    assert_eq!(
        emissive.iter().filter(|m| m.id < EMISSIVE_FIRST).count(),
        1,
        "one emissive row lives outside the block (Lava)"
    );
    let gain = cat
        .stage
        .as_ref()
        .expect("the catalog compiled its stage")
        .emissive_gain;
    for (i, colour) in palette.iter().enumerate() {
        let slot = EMISSIVE_FIRST + i as u8;
        let row = cat
            .materials
            .iter()
            .find(|m| m.id == slot)
            .unwrap_or_else(|| panic!("emissive slot {slot} holds a row"));
        assert_eq!(row.render_class, Some(RenderClass::Emissive), "slot {slot}");
        assert_eq!(row.color, colour.rgb, "slot {slot} wears palette entry {i}");
        let sample = cat
            .samples
            .iter()
            .find(|s| s.slot == slot)
            .unwrap_or_else(|| panic!("slot {slot}: an assigned glow has a sphere"));
        assert_eq!(
            sample.flat.emissive,
            Some(colour.rgb),
            "slot {slot}: the flat sphere emits its palette colour"
        );
        assert_eq!(
            sample.flat,
            FlatMaterial::of(row),
            "slot {slot}: wears its row"
        );
    }
    assert!(gain > 1.0, "the glow burns past unity: {gain}");
}

/// The one declared intent: Menu opens the pause overlay — the root declares it and the
/// footer's button fires the same name (one signal name, every trigger).
#[test]
fn the_menu_intent_is_declared_and_reaches_the_footer() {
    let tree = tree_of(&shipped());
    assert_eq!(
        tree.props.get("on_menu"),
        Some(&Value::Text("pause_open".into())),
        "the root declares Menu"
    );
    let mut all = Vec::new();
    flatten(&tree, &mut all);
    let menu = all
        .iter()
        .find(|n| n.id == "mc_footer_menu")
        .expect("the footer's Menu button");
    assert_eq!(menu.action.as_deref(), Some("pause_open"));
    assert_eq!(menu.tab_group, "mc_footer", "pad-reachable");
    assert!(menu.nav_ordinal > 0);
}

// ── the laid-out catalog ──────────────────────────────────────────────────────

/// Laid out at 1080p, the catalog is sixteen boxes across and sixteen down inside the
/// panel: slot 1 right of slot 0, slot 16 below it, every box the same real size, every
/// assigned material's name drawn under its box, the emissive block's word drawn eight
/// times, and no raw token anywhere — and every box reserves its sphere's seat inside its
/// cell, a poster under the sample stage.
#[test]
fn the_catalog_lays_out_sixteen_by_sixteen_at_1080p() {
    load_strings();
    let cat = shipped();
    let tree = tree_of(&cat);
    let frame = run_ui(
        &tree,
        &cat.model(),
        &cat.ui_styles,
        &snap_1080p(),
        &mut UiState::new(),
    );

    let panel = frame.rect("mc_panel").expect("the panel is laid out");
    let r = |slot: MaterialId| {
        frame
            .rect(&cell_id(slot))
            .unwrap_or_else(|| panic!("slot {slot} has no rect"))
    };
    let (r0, r1, r16, r255) = (r(0), r(1), r(16), r(255));
    assert!(
        r0.size.x > 40.0 && r0.size.y > 24.0,
        "a box has real extent: {:?}",
        r0.size
    );
    assert!(
        (r1.pos.y - r0.pos.y).abs() < 0.5 && r1.pos.x >= r0.pos.x + r0.size.x - 0.5,
        "slot 1 sits right of slot 0: {:?} vs {:?}",
        r1.pos,
        r0.pos
    );
    assert!(
        (r16.pos.x - r0.pos.x).abs() < 0.5 && r16.pos.y >= r0.pos.y + r0.size.y - 0.5,
        "slot 16 sits below slot 0: {:?} vs {:?}",
        r16.pos,
        r0.pos
    );
    assert!(
        r255.pos.x > r16.pos.x && r255.pos.y > r1.pos.y,
        "slot 255 is the bottom-right box"
    );
    let (px1, py1) = (
        panel.pos.x + panel.size.x + 0.5,
        panel.pos.y + panel.size.y + 0.5,
    );
    for slot in 0..=MaterialId::MAX {
        let c = r(slot);
        assert!(
            (c.size.x - r0.size.x).abs() < 1.0 && (c.size.y - r0.size.y).abs() < 1.0,
            "slot {slot} is the same size as slot 0: {:?} vs {:?}",
            c.size,
            r0.size
        );
        assert!(
            c.pos.x >= panel.pos.x - 0.5
                && c.pos.y >= panel.pos.y - 0.5
                && c.pos.x + c.size.x <= px1
                && c.pos.y + c.size.y <= py1,
            "slot {slot} sits inside the panel"
        );
    }

    let drawn: Vec<&str> = frame
        .commands
        .iter()
        .filter_map(|c| match c {
            HudCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    for m in &cat.materials {
        assert!(
            drawn.contains(&m.name.as_str()),
            "{} is drawn under its box",
            m.name
        );
    }
    let emissive = strings::resolve("$mc_emissive");
    let open = (EMISSIVE_FIRST..=MaterialId::MAX)
        .filter(|&slot| cat.materials.iter().all(|d| d.id != slot))
        .count();
    assert_eq!(
        drawn.iter().filter(|t| **t == emissive.as_ref()).count(),
        open,
        "the held word is drawn once per OPEN emissive slot (none: the glows fill them)"
    );
    assert!(
        !drawn.iter().any(|t| t.starts_with('$')),
        "no unresolved token on the catalog"
    );

    // Every box reserves its sphere's seat — inside its own cell, above the name row, as a
    // poster under the sample stage. (The free and emissive boxes reserve one too: their
    // sample simply never comes, and the well shows.)
    assert_eq!(frame.surfaces.len(), SLOTS, "every box reserves a surface");
    let seat0 = frame.surface(&box_id(0)).expect("slot 0's box is seated");
    for slot in 0..=MaterialId::MAX {
        let seat = frame
            .surface(&box_id(slot))
            .unwrap_or_else(|| panic!("slot {slot}'s box reserved no surface"));
        assert_eq!(seat.source, SAMPLE_STAGE_SOURCE, "slot {slot}");
        assert_eq!(seat.rate, Rate::Poster, "slot {slot}");
        let c = r(slot);
        assert!(
            seat.w > 40.0 && seat.h > 16.0,
            "slot {slot}: the sphere has room: {}×{}",
            seat.w,
            seat.h
        );
        assert!(
            (seat.w - seat0.w).abs() < 0.5 && (seat.h - seat0.h).abs() < 0.5,
            "slot {slot}: every seat is one size (one bloom scratch, one framing)"
        );
        assert!(
            seat.x >= c.pos.x - 0.5
                && seat.y >= c.pos.y - 0.5
                && seat.x + seat.w <= c.pos.x + c.size.x + 0.5
                && seat.y + seat.h < c.pos.y + c.size.y,
            "slot {slot}: the seat sits in its cell, above the name row"
        );
    }
}

// ── a generated slot ──────────────────────────────────────────────────────────

/// The size a bundle's maps decode to is AUTHORED (`params.sample_map_px`, Aaron's to tune)
/// and read through one reader: a whole number above zero, else `None` — warned, and the
/// bundles wear their flat stand-in.
#[test]
fn the_decode_size_is_authored_and_a_bad_one_is_refused() {
    let px = sample_map_px(&shipped_def().params).expect("the scene authors the decode size");
    assert!(px > 0);
    let params = |v: serde_json::Value| {
        let mut m = serde_json::Map::new();
        m.insert(MAP_PX_PARAM.to_string(), v);
        m
    };
    assert_eq!(sample_map_px(&params(serde_json::json!(96))), Some(96));
    assert_eq!(
        sample_map_px(&params(serde_json::json!(64.0))),
        Some(64),
        "a whole number spelled as a float is the same number"
    );
    for bad in [
        serde_json::json!(0),
        serde_json::json!(-8),
        serde_json::json!(12.5),
        serde_json::json!("big"),
    ] {
        assert_eq!(
            sample_map_px(&params(bad.clone())),
            None,
            "{bad} is refused"
        );
    }
    assert_eq!(
        sample_map_px(&serde_json::Map::new()),
        None,
        "unauthored is refused"
    );
}

/// A GENERATED slot — a bundle the package holds (the P3 DEFINED predicate) — arrives as
/// its SEVEN maps: the catalog finds the bundle, the worker decodes it through
/// `decode_bundle_maps` at the authored size (six maps decoded, each under its own kind;
/// Height required on disk and never decoded — no shading slot reads it), and the tally
/// counts it. Driven against a real bundle written into a scratch package root (seven PNG
/// maps, each its own colour, and the recipe, in the one layout `bundle_dir` spells).
#[test]
fn a_generated_bundle_arrives_as_its_seven_maps() {
    let px = sample_map_px(&shipped_def().params).expect("the scene authors the decode size");
    let (root, slot, bundle) = scratch_bundle("maps");

    let staging = package_scratch("maps_staging");
    let mut cat = MaterialCatalog::shipped_with_roots(root.clone(), staging.clone());
    assert_eq!(cat.generated, vec![slot], "exactly that slot is generated");
    assert_eq!(cat.model().text("mc_generated"), Some("1"));
    assert!(
        cat.samples.iter().any(|s| s.slot == slot),
        "the generated slot is assigned, so its sphere exists to wear the maps"
    );

    // The worker hands the decoded maps to the frame thread.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while cat.take_decoded() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(cat.pending.len(), 1, "one bundle's maps arrive");
    let d = &cat.pending[0];
    assert_eq!(d.slot, slot);
    assert_eq!(d.maps.px(), px, "decoded at the authored size");
    for (kind, [r, g, b]) in MapKind::ALL.into_iter().zip(MAP_COLOURS) {
        let map = d.maps.get(kind);
        if kind == MapKind::Height {
            assert!(map.is_empty(), "Height is required on disk, never decoded");
            continue;
        }
        assert_eq!(map.len(), (px * px * 4) as usize, "{kind:?} is px² RGBA8");
        assert_eq!(&map[..4], &[r, g, b, 255], "{kind:?} is its own map");
    }
    let direct = flicker_materialview::decode_bundle_maps(&bundle, px).expect("the bundle decodes");
    for kind in MapKind::ALL {
        assert_eq!(
            direct.get(kind),
            d.maps.get(kind),
            "{kind:?}: the worker decoded the same maps the same way"
        );
    }

    // The worker has delivered, so nothing is writing into the scratch root any more.
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&staging);
}

/// LEAVING never waits on a queue of decodes: the scene's cancel flag (set by `exit`,
/// through `cancel_decodes`) makes a decode job that has not started start nothing — and
/// one that has finished deliver nothing — while an uncancelled job delivers its maps. The
/// job itself is the one `build` hands the pool, run here on this thread.
#[test]
fn a_cancelled_decode_starts_nothing_and_delivers_nothing() {
    let px = sample_map_px(&shipped_def().params).expect("the scene authors the decode size");
    let (root, slot, bundle) = scratch_bundle("cancel");
    let (tx, rx) = mpsc::channel();

    decode_job(slot, &bundle, px, &AtomicBool::new(true), &tx);
    assert!(rx.try_recv().is_err(), "a cancelled job sends nothing");

    decode_job(slot, &bundle, px, &AtomicBool::new(false), &tx);
    let d = rx.try_recv().expect("an uncancelled job delivers its maps");
    assert_eq!((d.slot, d.maps.px()), (slot, px));

    // `exit` sets the very flag every queued job reads.
    let cat = shipped();
    assert!(
        !cat.cancel.load(Ordering::Acquire),
        "a live catalog decodes"
    );
    cat.cancel_decodes();
    assert!(cat.cancel.load(Ordering::Acquire), "a departed one cancels");

    let _ = std::fs::remove_dir_all(&root);
}

/// The catalog checks its sample stage ONCE, at load, through the two shared predicates —
/// the sample's own (`sample_stage_problems`) and the HDR resolve the shipped-stage gate
/// holds every lit stage to (`hdr_resolve_problems`) — and names every breach against
/// `stages.mc_sample`: no camera, no `material` layer, a layer the sample cannot draw, two
/// tonemaps; a stage with no rgba16f at all is not HDR. The shipped stage passes, which is
/// why the catalog kept it.
#[test]
fn a_broken_sample_stage_is_named_once_against_mc_sample() {
    let cat = shipped();
    let shipped_stage = cat
        .stage
        .as_ref()
        .expect("the shipped stage passes and is kept");
    assert!(sample_stage_check(shipped_stage, SAMPLE_STAGE_SOURCE).is_empty());

    let styles = serde_json::json!({ "stages": {
        "broken": {
            "layers": [{ "draw": "grid" }],
            "attachments": { "hdr": { "format": "rgba16f" },
                             "color": { "format": "surface" },
                             "depth": { "format": "depth32" } },
            "passes": [ { "pass": "scene", "writes": ["hdr", "depth"] },
                        { "pass": "tonemap_grade" },
                        { "pass": "tonemap_grade" } ]
        },
        "flat": { "layers": [{ "draw": "material" }], "camera": { "dist": 2.2 } }
    } });
    let compiled = |source: &str| {
        flicker::ui::compile_stage(&styles, source)
            .expect("authored")
            .0
    };
    let broken = sample_stage_check(&compiled("broken"), SAMPLE_STAGE_SOURCE);
    let text = broken.join("\n");
    for expected in [
        "`camera`",
        "no `material` layer",
        "[\"grid\"] layers",
        "EXACTLY one tonemap_grade (found 2)",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
    assert!(
        broken
            .iter()
            .all(|p| p.contains(&format!("stages.{SAMPLE_STAGE_SOURCE}"))),
        "every breach names the catalog's stage: {broken:?}"
    );
    let flat = sample_stage_check(&compiled("flat"), SAMPLE_STAGE_SOURCE);
    assert_eq!(flat.len(), 1, "{flat:?}");
    assert!(flat[0].contains("no rgba16f `hdr` attachment"), "{flat:?}");
}

/// The glow reads the SAME in the catalog as in the MaterialBench: the two stages' bloom
/// passes carry equal threshold, knee, intensity and radius, and both author the same
/// emissive gain. Their FRAMING may differ — the catalog's boxes are small, so its camera is
/// deliberately tighter.
#[test]
fn the_catalog_blooms_exactly_as_the_bench_does() {
    let cat = shipped();
    let (ours, _) = flicker::ui::compile_stage(&cat.ui_styles, SAMPLE_STAGE_SOURCE)
        .expect("the catalog authors its stage");
    let bench_scene = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/sensorium/scenes/materialbench.scene.json"
    ))
    .expect("the bench's scene file reads");
    let bench_def =
        SceneDef::parse("materialbench", &bench_scene).expect("the bench's scene file parses");
    let bench_styles = flicker::ui::load_shared_styles(bench_def.styles.as_ref());
    let (theirs, _) = flicker::ui::compile_stage(&bench_styles, "materialbench_lit")
        .expect("the bench authors its lit stage");
    assert_eq!(
        bloom_of(&ours, SAMPLE_STAGE_SOURCE),
        bloom_of(&theirs, "materialbench_lit"),
        "the two lit stages bloom identically"
    );
    assert_eq!(
        ours.emissive_gain, theirs.emissive_gain,
        "and burn a glow at the same authored gain"
    );
    let (near, far) = (
        ours.camera.expect("framed").dist,
        theirs.camera.expect("framed").dist,
    );
    assert!(near < far, "the catalog frames tighter: {near} vs {far}");
}

/// The HUD's texture table is the THEME's, and the footer's glyph atlas is found in it by
/// name: the authored `pad_glyphs.tex` is the index of `"pad_glyphs"` in the theme's texture
/// names (`Theme::TEXTURE_NAMES`, what `lua_textures` hands out) — so a change to the table
/// can never silently point the glyphs at another texture again (they once indexed slot 2's
/// swatch).
#[test]
fn the_footer_glyphs_index_the_themes_glyph_atlas() {
    let cat = shipped();
    let authored = cat.ui_styles["pad_glyphs"]["tex"]
        .as_u64()
        .expect("the scene authors pad_glyphs.tex");
    let atlas = Theme::TEXTURE_NAMES
        .iter()
        .position(|n| *n == "pad_glyphs")
        .expect("the theme hands out a pad_glyphs texture");
    assert_eq!(authored as usize, atlas);
}

// ── the pressable boxes ───────────────────────────────────────────────────────

/// EVERY BOX OPENS THE INSPECTOR ON ITS OWN SLOT: the name a box fires (`mc_open_<id>`, the
/// grid gate pins it on the authored panel) reads back as exactly that slot through the
/// dispatcher's one reader, and the inspector built for it is that slot's — every slot of
/// the byte, a FREE one included (never a dead click). A name that names no slot opens
/// nothing.
#[test]
fn every_box_opens_the_inspector_on_its_own_slot() {
    let cat = shipped();
    for slot in 0..=MaterialId::MAX {
        let mut results = ValueMap::new();
        results.set("hud_hit", true);
        results.set(open_action(slot), true);
        assert_eq!(opened_slot(&results), Some(slot), "slot {slot}");
    }
    for slot in [0, 10, 67, 200, 248, 255] {
        let insp = cat.inspect(slot);
        assert_eq!(insp.facts().slot, slot);
        assert_eq!(
            insp.facts().row.is_some(),
            cat.materials.iter().any(|m| m.id == slot),
            "slot {slot}: the row the index holds, or none"
        );
    }
    assert_eq!(opened_slot(&ValueMap::new()), None);
    let mut junk = ValueMap::new();
    junk.set("mc_open_256", true);
    junk.set("mc_open_x", true);
    junk.set("mc_open_7", false);
    assert_eq!(
        opened_slot(&junk),
        None,
        "nothing that names a pressed slot"
    );
}

/// THE PAD REACHES A BOX AND CONFIRM OPENS IT — through the real walker over the shipped
/// tree: the first d-pad press lands in the grid pane on slot 0 (the pane is the stick's
/// first stop, the lowest-ordinal box its first control), the d-pad steps the byte in id
/// order, Confirm fires the focused box's own `mc_open_<id>`, and the stick steps out to
/// the footer pane and back.
#[test]
fn the_pad_reaches_a_box_and_confirm_opens_it() {
    use flicker_input_core::{ActionSignal, EventKind, InputContext};
    use flicker_input_router::{InputEvent, RouteCtx};

    let cat = shipped();
    let tree = tree_of(&cat);
    let model = cat.model();
    let intents = UiIntents::of(&tree);
    let raw = InputState::new();
    let mut ui = UiState::new();
    let mut rc = RouteCtx::new();
    let mut send = |ui: &mut UiState, signals: &[ActionSignal]| {
        let mut h = WalkerHandler::hud(ui, false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
        for &s in signals {
            h.handle(
                &InputEvent::new(s, EventKind::Press, InputContext::Menu, &raw),
                &mut rc,
            );
        }
        h.take_fired()
    };
    send(&mut ui, &[ActionSignal::NavDown]);
    assert_eq!(
        ui.focused(),
        Some("mc_slot_0"),
        "the first press lands on slot 0"
    );
    send(&mut ui, &[ActionSignal::NavRight, ActionSignal::NavRight]);
    assert_eq!(
        ui.focused(),
        Some("mc_slot_2"),
        "the d-pad walks the byte in id order"
    );
    let fired = send(&mut ui, &[ActionSignal::Confirm]);
    assert!(
        fired.contains(&open_action(2)),
        "Confirm fires the focused box's open: {fired:?}"
    );
    let mut results = ValueMap::new();
    for name in &fired {
        results.set(name.as_str(), true);
    }
    assert_eq!(opened_slot(&results), Some(2));
    send(&mut ui, &[ActionSignal::PanelNext]);
    assert_eq!(
        ui.focused(),
        Some("mc_footer_menu"),
        "the stick steps to the footer pane"
    );
    send(&mut ui, &[ActionSignal::PanelPrev]);
    assert_eq!(
        ui.focused(),
        Some("mc_slot_0"),
        "and back into the grid at its first box"
    );
}

/// THE PAD WALKS THE GRID AS IT IS LAID OUT (`nav: grid` on the slot pane): with the frame's
/// real rects — what `update` hands the walker — Down from slot 0 is the box BELOW it (slot
/// 16, the next row), Right the box beside it, Up and Left back; past the grid's edge the
/// d-pad carries on in id order. Through the real walker over the shipped tree.
#[test]
fn the_pad_walks_the_grid_as_it_is_laid_out() {
    use flicker_input_core::{ActionSignal, EventKind, InputContext};
    use flicker_input_router::{InputEvent, RouteCtx};

    let cat = shipped();
    let tree = tree_of(&cat);
    let model = cat.model();
    let intents = UiIntents::of(&tree);
    let mut ui = UiState::new();
    let frame = run_ui(&tree, &model, &cat.ui_styles, &snap_1080p(), &mut ui);
    let raw = InputState::new();
    let mut rc = RouteCtx::new();
    let mut step = |ui: &mut UiState, from: MaterialId, signal: ActionSignal| {
        ui.request_focus(cell_id(from));
        WalkerHandler::hud(ui, false)
            .with_nav(&tree, &model)
            .with_rects(&frame.rects)
            .with_intents(&intents)
            .handle(
                &InputEvent::new(signal, EventKind::Press, InputContext::Menu, &raw),
                &mut rc,
            );
        ui.focused().map(str::to_string)
    };
    let cols = GRID_COLS as MaterialId;
    for (from, signal, to, what) in [
        (
            0,
            ActionSignal::NavDown,
            cols,
            "Down from slot 0 is the box below it",
        ),
        (
            cols,
            ActionSignal::NavRight,
            cols + 1,
            "Right is the box beside it",
        ),
        (cols + 1, ActionSignal::NavUp, 1, "Up is the box above it"),
        (1, ActionSignal::NavLeft, 0, "Left is the box beside it"),
        (
            37,
            ActionSignal::NavDown,
            53,
            "a column holds anywhere in the grid",
        ),
        (
            cols - 1,
            ActionSignal::NavRight,
            cols,
            "past a row's end: on in id order",
        ),
    ] {
        assert_eq!(step(&mut ui, from, signal), Some(cell_id(to)), "{what}");
    }
}

/// DEVELOPMENT-TIER GATES: tests that read this crate's own source.
mod gates {
    use super::*;

    /// Every place `src` composes, edits or MUTABLY reaches a UI node, as `(line, why)`.
    ///
    /// Comments and `use` statements aside, the code is read WHITESPACE-FREE, as one run —
    /// so `tree.props .insert(` and a chain rustfmt split across lines read as what they
    /// are. A breach is any of:
    /// - `UiNode` named outside a type that only HOLDS a tree (`Option<UiNode>`,
    ///   `Vec<UiNode>`, `&UiNode`, a `tree: UiNode,` field) — `&mut UiNode`, a literal, a
    ///   turbofish, a `UiNode::` path all edit or make one;
    /// - a node's `.children` or `.props` reached other than through a READ-ONLY method
    ///   (`iter` `len` `is_empty` `get` `contains_key` `first` `last` `keys` `values`), or
    ///   copied out of one (`.cloned()` / `.clone()` / `.to_vec()` / `.to_owned()` after it);
    /// - `&mut` reaching ANY field of a node, or the held tree itself (`mem::swap` / `take` /
    ///   `replace`);
    /// - the held tree REPLACED: assigned anything but the catalog's own put-back
    ///   (`self.tree = Some(tree);` after its `self.tree.take()`), swapped by an `Option`
    ///   method other than a read (`take` `as_ref` `is_some` `is_none` `clone`), overwritten
    ///   by `clone_from`, or written through a dereference (`*tree = …`);
    /// - a write to any field of a node (`.action =`, `.tab_group =`, `.nav_ordinal +=`, …),
    ///   which a comparison (`==`, `<=`) is not;
    /// - a parse or a builder (`parse_ui_json`, `.ui_tree(`, `apply_props(`, `chat_panel(`,
    ///   `children: vec!`), or the walker's repeater handed a MUTABLE tree — its sanctioned
    ///   use takes the authored tree by `&` and returns the expanded one.
    fn composition_breaches(src: &str) -> Vec<(usize, String)> {
        const HELD: [&str; 4] = ["Option<UiNode>", "Vec<UiNode>", "&UiNode", "tree:UiNode,"];
        const FIELDS: [&str; 20] = [
            "id",
            "component",
            "children",
            "anchor",
            "offset",
            "size",
            "grow",
            "width",
            "height",
            "gap",
            "pad",
            "pad_x",
            "pad_y",
            "props",
            "bind",
            "action",
            "visible_bind",
            "enabled_bind",
            "nav_ordinal",
            "tab_group",
        ];
        const READS: [&str; 9] = [
            "iter",
            "len",
            "is_empty",
            "get",
            "contains_key",
            "first",
            "last",
            "keys",
            "values",
        ];
        const COPIES: [&str; 4] = [".cloned()", ".clone()", ".to_vec()", ".to_owned()"];
        const ASSIGN: [&str; 10] = ["+=", "-=", "*=", "/=", "%=", "|=", "&=", "^=", "<<=", ">>="];
        const MAKES: [&str; 6] = [
            "children:vec!",
            "parse_ui_json",
            ".ui_tree(",
            "apply_props(",
            "chat_panel(",
            "instantiate_rows(&mut",
        ];

        // The code as one whitespace-free run, each byte remembering its source line.
        let (mut code, mut line_at) = (String::new(), Vec::new());
        let mut in_use = false;
        for (n, line) in src.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if t.starts_with("use ")
                || t.starts_with("pub use ")
                || t.starts_with("pub(crate) use ")
            {
                in_use = true;
            }
            if in_use {
                in_use = !t.contains(';');
                continue;
            }
            for c in line.chars().filter(|c| !c.is_whitespace()) {
                code.push(c);
                line_at.extend(std::iter::repeat_n(n + 1, c.len_utf8()));
            }
        }
        let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let mut out = Vec::new();
        let mut breach = |at: usize, why: String| out.push((line_at[at], why));

        // A node named outside a type that holds one.
        let mut held = code.clone();
        for h in HELD {
            held = held.replace(h, &" ".repeat(h.len()));
        }
        for (at, _) in held.match_indices("UiNode") {
            breach(at, "names `UiNode` outside a type that holds a tree".into());
        }
        // Children and props: read-only methods only, nothing copied out.
        for member in [".children", ".props"] {
            for (at, _) in code.match_indices(member) {
                let rest = &code[at + member.len()..];
                if rest.starts_with(ident) {
                    continue; // `.children_of(`, `.propsx` — another name
                }
                let Some(call) = rest.strip_prefix('.') else {
                    breach(at, format!("reaches `{member}` other than through a read"));
                    continue;
                };
                let method: String = call.chars().take_while(|&c| ident(c)).collect();
                if !READS.contains(&method.as_str()) {
                    breach(at, format!("`{member}.{method}` — not a read-only method"));
                    continue;
                }
                // The read's own call, then whatever unwraps it, must not copy a node out.
                let mut tail = &call[method.len()..];
                if tail.starts_with('(') {
                    let mut depth = 0usize;
                    let end = tail
                        .char_indices()
                        .find_map(|(i, c)| {
                            match c {
                                '(' => depth += 1,
                                ')' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        return Some(i + 1);
                                    }
                                }
                                _ => {}
                            }
                            None
                        })
                        .unwrap_or(tail.len());
                    tail = &tail[end..];
                }
                while let Some(t) = tail
                    .strip_prefix(".unwrap()")
                    .or_else(|| tail.strip_prefix('?'))
                {
                    tail = t;
                }
                if COPIES.iter().any(|c| tail.starts_with(c)) {
                    breach(at, format!("copies a node out of `{member}`"));
                }
            }
        }
        // `&mut` reaching ANY field of a node — or the held tree itself (`mem::take`,
        // `mem::replace`, `mem::swap` on `self.tree`).
        let names_field = |operand: &str, field: &str| {
            operand.match_indices(&format!(".{field}")).any(|(i, m)| {
                !operand[i + m.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            })
        };
        for (at, _) in code.match_indices("&mut") {
            let operand: String = code[at + 4..]
                .chars()
                .take_while(|&c| c != ',' && c != ')' && c != ';')
                .collect();
            let field = FIELDS.iter().find(|f| names_field(&operand, f));
            if field.is_some() || names_field(&operand, "tree") {
                breach(
                    at,
                    format!("`&mut {operand}` reaches into a node or the tree"),
                );
            }
        }
        // The held tree REPLACED: an assignment other than the catalog's own put-back
        // (`self.tree = Some(tree);`, after its `self.tree.take()`), an Option method that
        // swaps it, a `clone_from` over a node, or a write through a dereference.
        const TREE_READS: [&str; 5] = ["take", "as_ref", "is_some", "is_none", "clone"];
        for (at, _) in code.match_indices(".tree") {
            let rest = &code[at + ".tree".len()..];
            if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                continue; // `.tree_of(`, `.trees` — another name
            }
            if let Some(value) = rest.strip_prefix('=').filter(|v| !v.starts_with('=')) {
                if !(code[..at].ends_with("self") && value.starts_with("Some(tree);")) {
                    breach(at, "replaces the held tree".into());
                }
            } else if let Some(call) = rest.strip_prefix('.') {
                let method: String = call.chars().take_while(|&c| ident(c)).collect();
                if !TREE_READS.contains(&method.as_str()) {
                    breach(
                        at,
                        format!("`.tree.{method}` — not a read of the held tree"),
                    );
                }
            }
        }
        for spelling in [".clone_from(", "*tree=", "*node="] {
            for (at, _) in code.match_indices(spelling) {
                breach(at, format!("`{spelling}` replaces a tree or a node"));
            }
        }
        // A write to a node's field.
        for field in FIELDS {
            let dotted = format!(".{field}");
            for (at, _) in code.match_indices(&dotted) {
                let rest = &code[at + dotted.len()..];
                let plain = rest.starts_with('=') && !rest.starts_with("==");
                if plain || ASSIGN.iter().any(|op| rest.starts_with(op)) {
                    breach(at, format!("writes a node's `{field}`"));
                }
            }
        }
        for make in MAKES {
            for (at, _) in code.match_indices(make) {
                breach(at, format!("`{make}` makes or edits a tree"));
            }
        }
        if let Some(at) = code.find("children[") {
            breach(at, "indexes a node's children".into());
        }
        out.sort();
        out
    }

    /// COMPOSITION IS DATA (the MaterialBench's `the_bench_composes_no_ui_node`, for this
    /// crate): neither the catalog nor its inspector builds, edits or parses a UI node, or
    /// so much as reaches one mutably — the trees are the scene file's and the shared
    /// file's, loaded through the manifest's def and the shell's hosted-tree path, and the
    /// inspector's node list is expanded by the walker's own repeater from the AUTHORED tree
    /// (`instantiate_rows(&…)`). A `UiNode` is only ever HELD here.
    #[test]
    fn the_catalog_composes_no_ui_node() {
        const SHIPPED: [(&str, &str); 2] = [
            ("lib.rs", include_str!("lib.rs")),
            ("inspector.rs", include_str!("inspector.rs")),
        ];
        let mut breaches = Vec::new();
        let mut scene_parses = 0;
        for (file, src) in SHIPPED {
            let shipped = src.split("#[cfg(test)]\nmod tests;").next().unwrap_or(src);
            breaches.extend(
                composition_breaches(shipped)
                    .into_iter()
                    .map(|(line, why)| format!("{file}:{line} {why}")),
            );
            scene_parses += shipped.matches("SceneDef::parse(").count();
        }
        assert!(
            breaches.is_empty(),
            "the catalog composes UI:\n{}",
            breaches.join("\n")
        );
        assert_eq!(
            scene_parses, 1,
            "the one shipped-scene parse (the test seam), no other"
        );
        let inspector = include_str!("inspector.rs");
        assert!(
            inspector.contains("hosted_modal_tree(INSPECTOR_JSON, INSPECTOR_ID)"),
            "the inspector's tree loads through the shell's one hosted-tree path"
        );
    }

    /// THE GATE CATCHES WHAT IT CLAIMS TO: every spelling a review found slipping the
    /// earlier, needle-based version — mutable access, a field write, a copy out of a
    /// node, a spacing trick, a split chain — is a breach, and the sanctioned reads are not.
    #[test]
    fn the_composition_gate_catches_every_known_spelling() {
        const COMPOSES: [&str; 37] = [
            "fn sneak(tree: &mut UiNode) {",
            "tree.children.swap(0, 1);",
            "tree.children.pop();",
            "tree.children.remove(0);",
            "tree.children.sort_by(|a, b| a.id.cmp(&b.id));",
            "tree.children.reverse();",
            "tree.children.dedup_by(|a, b| a.id == b.id);",
            "tree.children.iter_mut().for_each(|c| c.gap = 2.0);",
            "tree.children.first_mut().unwrap().action = Some(String::new());",
            "tree.props.remove(\"style\");",
            "tree.props.retain(|k, _| k != \"x\");",
            "tree.props.entry(\"label\".into()).or_insert(Value::Text(\"$x\".into()));",
            "tree.props.extend(other.props.clone());",
            "*tree.props.get_mut(\"label\").unwrap() = Value::Text(\"x\".into());",
            "tree.tab_group = \"g\".into();",
            "tree.nav_ordinal = 3;",
            "tree.action = Some(\"x\".into());",
            "tree.bind = Some(\"x\".into());",
            "tree.visible_bind = Some(\"x\".into());",
            "tree.id = \"x\".into();",
            "tree.id=\"x\".into();",
            "tree.size = Some(10.0);",
            "std::mem::swap(&mut tree.children, &mut kids);",
            "let kids = std::mem::take(&mut tree.children);",
            "let mut t2 = tree.clone(); t2.component = \"cell\".into();",
            "let n: flicker::script::UiNode = Default::default();",
            "let n = serde_json::from_value::<flicker::script::UiNode>(v);",
            "let n = flicker::script::parse_ui_json(&v);",
            "let mut n = tree.children[0].clone();",
            "let n = tree.children.get(0).cloned().unwrap();",
            "let mut n = ui_default(); n.children.push(x);",
            "let m = flicker::ui::rows::instantiate_rows(&mut tree, &model);",
            "flicker::ui::instantiate_rows (&mut t, &m);",
            "let c = node.children.clone(); node.children.clear();",
            "tree.children.clear();",
            "tree.children.insert (0, n);",
            "tree.props .insert(\"k\".into(), v);",
        ];
        for spelling in COMPOSES {
            assert!(
                !composition_breaches(spelling).is_empty(),
                "the gate lets `{spelling}` through"
            );
        }
        // The re-check's spellings: `&mut` to ANY field, and the held tree replaced whole.
        for spelling in [
            "let a = &mut tree.action;",
            "std::mem::swap(&mut node.tab_group, &mut group);",
            "self.tree = Some(other);",
            "let old = std::mem::take(&mut self.tree);",
            "let old = std::mem::replace(&mut self.tree, Some(t));",
            "self.tree.clone_from(&other);",
            "tree.clone_from(&other);",
            "self.tree.replace(other);",
            "self.tree.insert(other);",
            "*tree = other;",
            "let s = &mut tree.size;",
        ] {
            assert!(
                !composition_breaches(spelling).is_empty(),
                "the gate lets `{spelling}` through"
            );
        }
        // Split across lines the way rustfmt splits a long chain.
        assert!(!composition_breaches("tree\n    .children\n    .push(n);").is_empty());
        assert!(!composition_breaches("t.nav_ordinal += 1;").is_empty());
        // The sanctioned reads and holds are not breaches.
        for fine in [
            "let tree = instantiate_rows(&hosted, &mut model, &|s| rows(s));",
            "let n = tree.children.len();",
            "if tree.props.contains_key(\"pane\") {",
            "for c in tree.children.iter() {",
            "if r.id == slot {",
            "pub fn tree(&self) -> &UiNode {",
            "tree: Option<UiNode>,",
            "let Some(tree) = self.tree.take() else {",
            "self.tree = Some(tree);",
            "let frame = run_ui(&self.tree, &self.model, &self.styles, input, &mut self.ui_state);",
            "let walker = WalkerHandler::hud(&mut self.ui_state, hit).with_nav(&self.tree, &m);",
            "pub fn authored_tree(&self) -> Option<UiNode> { self.tree.clone() }",
        ] {
            assert!(
                composition_breaches(fine).is_empty(),
                "`{fine}` is a read, not a breach: {:?}",
                composition_breaches(fine)
            );
        }
    }

    /// The dispatcher pushes exactly what the box fired: `update` reads the slot through the
    /// one reader (`opened_slot`) and pushes the inspector built for THAT slot
    /// (`inspect`) — the two halves the production gate above drives headlessly, glued here.
    #[test]
    fn the_dispatcher_pushes_the_inspector_on_the_slot_the_box_fired() {
        let lib = include_str!("lib.rs");
        assert!(
            lib.contains(
                "if let (Some(slot), Some(theme)) = (opened_slot(&results), self.theme) {"
            ) && lib.contains(
                "return Transition::Push(Box::new(self.inspect(slot).with_theme(theme)));"
            ),
            "update pushes the inspector on the slot the box fired"
        );
    }

    /// Rust never publishes a style path — the trees name their own blocks.
    #[test]
    fn rust_never_publishes_a_style_path() {
        for (file, src) in [
            ("lib.rs", include_str!("lib.rs")),
            ("inspector.rs", include_str!("inspector.rs")),
        ] {
            for (n, line) in src.lines().enumerate() {
                if line.contains(".set(") {
                    assert!(
                        !line.contains("\"catalog.") && !line.contains("\"inspector."),
                        "{file}:{} publishes a style path: {line}",
                        n + 1
                    );
                }
            }
        }
    }

    /// The STRINGS gate's blind side: copy published from RUST into the Model bypasses
    /// the tree gate entirely, so the crate self-gates its own source.
    #[test]
    fn no_raw_display_copy_is_published_into_the_model() {
        let flagged = strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(
            flagged.is_empty(),
            "lib.rs publishes raw display copy: {flagged:?}"
        );
    }
}
