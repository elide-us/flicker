//! Committing the designer's output into `staging/`.
//!
//! `source/ → [benches] → staging/ → [Content Manager promotes] → package/`
//!
//! MaterialBench is an ingest bench like MeshBench and PackBench, so it writes here
//! and stops. Nothing it produces is visible to the running game until someone
//! reviews it in the Content Manager and promotes it — that split is the whole
//! point of the staging tier, and this module does not have a `package/` path in
//! it anywhere: it WRITES only under the staging root it is handed. It also READS a
//! committed look back ([`graph_at`], [`committed`]) from whichever roots it is
//! handed — the staging tree, and the package tree a promotion moved the bundle to —
//! so the designer can open what it made; reading is not promoting.
//!
//! # The two at-rest forms, and why they differ
//!
//! - **The material document is TEXT** — the v2 [`MaterialGraph`] as pretty JSON
//!   (`"version": 2`), under the SAME file name the v1 recipe always had
//!   (`<id:03>.texture.json`). It goes through [`flicker_content::package`] and lands gz,
//!   exactly like every other staged text artifact, so a promotion is a plain byte move.
//!   Every reader takes it through [`flicker_texture::load_material_json`], which reads
//!   this and the versionless v1 recipe alike.
//! - **The maps are PNG**, and PNG is already compressed. Binary content stays
//!   raw in both trees (`GZIFY_EXTENSIONS` is `json`/`flight`/`epoch`, and
//!   `gzify_dir` skips images), which is why they are written with a plain file
//!   write. That is not a bypass of the gz seam — it is the no-gz half of the
//!   same rule, and gzipping them would make a promotion a transcode.
//!
//! # Layout mirrors `package/`
//!
//! `materials/<id:03>/<id:03>_<Map>.png` — the folder and the file stems are the
//! bound material's SLOT ID, zero-padded (plan 30FE7F58 P3). A promotion is then
//! the same relative path under a different root, and the map suffixes are the
//! content standard's own role names (`GolemBase_Low_BaseColor.png` is already
//! shaped this way).
//!
//! The display name is NOT in the path: it lives in the document and in
//! `materials.json`, so renaming a material is a pure data edit and never moves
//! a file. The one join both sides spell is [`flicker_content::bundle_dir`] —
//! this module calls it with the STAGING root, and still has no `package/` path
//! in it anywhere.

use std::io;
use std::path::{Path, PathBuf};

use flicker_materials::{MaterialId, RenderClass};
use flicker_texture::{bake_graph, load_material_json, GraphError, MapKind, MaterialGraph};

/// Why a commit was REFUSED before it baked or wrote anything.
///
/// Each refusal is named by a stringtable TOKEN, because the bench publishes
/// what it is told straight into `commit_status`: a Rust string here would be
/// English on every locale, past the STRINGS gate (which scans source
/// literals, not the copy a `String` carries at runtime). The specifics —
/// which recipe, which slot — go to the log, not the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `graph.material` is `None`: an id-keyed bundle has no folder to land in.
    Unbound,
    /// The bound slot may not hold the material its index row is — THE slot law,
    /// [`flicker_materials::slot_accepts`]: slot 0 is Air and holds nothing, and the
    /// emissive slots (`flicker_materials::EMISSIVE_FIRST..=255`, ruling 2026-10-20) hold
    /// emissive-class materials only.
    SlotClosed,
}

impl Refusal {
    /// The stringtable token the bench shows for this refusal.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Refusal::Unbound => "$mtl_commit_unbound",
            Refusal::SlotClosed => "$mtl_commit_slot_closed",
        }
    }
}

/// What can stop a commit: a refusal (nothing baked, nothing written), a graph that
/// cannot bake (nothing written), or an I/O failure while writing the bundle.
#[derive(Debug)]
pub enum CommitError {
    Refused(Refusal),
    /// The graph refused to bake — the node and port it names are what the author
    /// fixes. Raised before the folder exists, so a broken graph writes nothing.
    Graph(GraphError),
    Io(io::Error),
}

impl From<io::Error> for CommitError {
    fn from(e: io::Error) -> Self {
        CommitError::Io(e)
    }
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The token, so a log line reads exactly what the screen shows.
            CommitError::Refused(why) => f.write_str(why.token()),
            CommitError::Graph(e) => e.fmt(f),
            CommitError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CommitError {}

/// What a commit wrote.
///
/// `Debug` so a refusal test can say what a commit wrongly produced instead of
/// failing with an opaque type.
#[derive(Debug)]
pub struct Committed {
    /// The asset folder, under the staging root.
    pub dir: PathBuf,
    /// Every file written, in write order — the document last, so a reader that
    /// sees it knows the maps beside it are complete.
    pub files: Vec<PathBuf>,
}

/// Bake `graph` at `size` and write the bundle into `staging_root`.
///
/// `class` is the render class of the material INDEX row the graph's slot binds (`None`
/// when the index lists no row there) — what the slot law weighs the commit against.
/// `staging_root` is passed in rather than looked up so a test can drive this against a
/// temp tree; the scene asks `flicker_content::roots().staging()`.
///
/// # Refusals
///
/// The bundle is keyed by the graph's bound SLOT, so a graph that names no slot
/// has no correct folder to land in, and one bound to a slot that may not hold its
/// material has no legitimate one — Air's, or an emissive slot whose row is not an
/// emissive material. Both come back as [`CommitError::Refused`] BEFORE the bake runs
/// — a rejected commit must not cost the ~360 ms a baseline bake does — and each
/// carries a stringtable token, so what the bench shows is localized copy, never
/// English from here. A graph that cannot bake comes back as [`CommitError::Graph`]
/// before the folder is made.
pub fn commit(
    graph: &MaterialGraph,
    class: Option<RenderClass>,
    size: u32,
    staging_root: &Path,
) -> Result<Committed, CommitError> {
    let Some(slot) = graph.material else {
        return Err(CommitError::Refused(Refusal::Unbound));
    };
    // THE slot law, asked of the ONE predicate with the bound row's class: Air holds
    // nothing, and an emissive slot holds an emissive row only — so a non-emissive
    // material can never be committed into 248–255, and the boundary is never a
    // literal here.
    if !flicker_materials::slot_accepts(slot, class) {
        return Err(CommitError::Refused(Refusal::SlotClosed));
    }
    let set = bake_graph(graph, size).map_err(CommitError::Graph)?;

    // The ONE join the writer and the reader both spell.
    let dir = flicker_content::bundle_dir(staging_root, slot);
    std::fs::create_dir_all(&dir)?;
    // Every stem in the bundle is the slot, so a re-commit of the same slot
    // REPLACES it file for file rather than accumulating a second folder.
    let seg = flicker_materials::slot_dir(slot);
    let mut files = Vec::with_capacity(MapKind::ALL.len() + 1);

    for kind in MapKind::ALL {
        let Some(map) = set.get(kind) else { continue };
        let path = dir.join(format!("{seg}_{}.png", kind.role()));
        let png = encode_png(kind, &map.pixels, map.size).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{kind:?} PNG encode: {e}"),
            )
        })?;
        // Raw, not gz — see the module docs.
        std::fs::write(&path, png)?;
        files.push(path);
    }

    // The document LAST: it is the artifact that says "these maps exist", so a
    // reader that finds it can trust the folder beside it is finished.
    let json = serde_json::to_string_pretty(graph)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let doc = dir.join(format!("{seg}.texture.json"));
    files.push(flicker_content::package::write_text(&doc, &json)?);

    Ok(Committed { dir, files })
}

/// The graph the bundle under `root` holds for `slot` — the document [`commit`] wrote, read
/// back through the ONE material loader every reader uses ([`load_material_json`]) — or
/// `Ok(None)` when `root` holds no bundle for that slot. Root-agnostic: the staging tree
/// and the package tree spell a bundle the same way ([`flicker_content::bundle_dir`]).
///
/// The FOLDER is the slot (it is where the bundle lives and promotes), so the graph comes
/// back bound to `slot` even when its document names another or none — a hand-edited
/// bundle, the PackageBench's D-MISMATCH — and that disagreement is logged rather than
/// carried.
///
/// # Errors
///
/// A bundle whose document does not read or does not load, as data (its path and the
/// loader's reason) — the bundle is there, so silence would be wrong.
pub fn graph_at(root: &Path, slot: MaterialId) -> Result<Option<MaterialGraph>, String> {
    let Some(bundle) = flicker_content::bundle_at(root, slot) else {
        return Ok(None);
    };
    let text = bundle.recipe_text().map_err(|e| format!("{e:#}"))?;
    let mut graph =
        load_material_json(&text).map_err(|e| format!("{}: {e}", bundle.recipe.display()))?;
    if graph.material != Some(slot) {
        tracing::warn!(
            slot,
            claims = ?graph.material,
            dir = %bundle.dir.display(),
            "a committed material document names another slot than its folder — opened bound to the folder's"
        );
        graph.material = Some(slot);
    }
    Ok(Some(graph))
}

/// The COMMITTED look of `slot` the designer opens: the bundle in `staging_root` first —
/// the newest commit, not yet reviewed — else the one in `package_root`, where a promotion
/// MOVES it (so a promoted material still opens where it was made). `Ok(None)` when
/// neither tree holds one. Both roots come from the content roots service; neither is
/// spelled here.
///
/// # Errors
///
/// The first tree's bundle that will not load ([`graph_at`]) — a broken staged document
/// is named rather than passed over for an older promoted one.
pub fn committed(
    staging_root: &Path,
    package_root: &Path,
    slot: MaterialId,
) -> Result<Option<MaterialGraph>, String> {
    match graph_at(staging_root, slot)? {
        Some(graph) => Ok(Some(graph)),
        None => graph_at(package_root, slot),
    }
}

/// Read a LIST of authored material graphs — `{ "graphs": [ <document>, … ] }`, the
/// `_comment` keys beside it ignored — each entry through [`load_material_json`], exactly
/// as a bundle's document is read. The ONE reader of an authored library: the bench's
/// preset library reads `data/factory_materials.json` through it, and the headless commit
/// (`examples/commit_library.rs`) whichever library it is named — that file, or
/// `data/glow_materials.json`.
///
/// # Errors
///
/// A file that is not an object holding a `graphs` array, or the first entry that is not
/// a material document — named by its position and the loader's reason.
pub fn graphs_in(text: &str) -> Result<Vec<MaterialGraph>, String> {
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let entries = doc
        .get("graphs")
        .and_then(serde_json::Value::as_array)
        .ok_or("the document holds no `graphs` array")?;
    entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            load_material_json(&entry.to_string()).map_err(|e| format!("graphs[{i}]: {e}"))
        })
        .collect()
}

/// Encode one map to PNG, in the **narrowest form that carries its meaning**.
///
/// The baker works in RGBA8 because that is what the GPU upload path takes, but
/// most of those channels hold nothing: a roughness map is one number repeated
/// three times under an alpha that is always 255. Writing that verbatim costs
/// roughly double the artifact for no information — ~19 MB per material at 2K
/// instead of ~10 — and every one of those bytes gets reviewed, promoted and
/// shipped.
///
/// So: the scalar maps go out as **L8** (their red channel, which is the value),
/// and the three-channel maps as **RGB8** (the bake guarantees an opaque alpha, and
/// `every_baked_map_is_opaque` in `flicker-texture` is what lets this drop it).
/// A decoder widens back to RGBA8 on load; PNG carries its own channel count, so
/// nothing has to be told which form a file is in.
fn encode_png(kind: MapKind, rgba: &[u8], size: u32) -> Result<Vec<u8>, image::ImageError> {
    // Three-channel maps are the COLOURS (`is_color`) plus `Normal`, which is a
    // vector rather than a colour but still needs all three components. Everything
    // else is one number replicated, and shipping THOSE as RGB8 wastes two thirds
    // of the file — while shipping a colour as L8 destroys its hue, which is how
    // an `Emit` map would have lost the whole point of being a colour.
    let (color, pixels) = match kind {
        _ if kind.is_color() || kind == MapKind::Normal => (
            image::ColorType::Rgb8,
            rgba.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect::<Vec<u8>>(),
        ),
        // Scalar: one channel is the whole map.
        _ => (
            image::ColorType::L8,
            rgba.as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<u8>>(),
        ),
    };
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut out,
        &pixels,
        size,
        size,
        color,
        image::ImageFormat::Png,
    )?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_content::PackageClass;
    use flicker_texture::{load_material_json, presets, TextureRecipe};

    /// Unique per process (pid) and per call (atomic counter), so two concurrent
    /// `cargo test` runs never share a fixed dir and stomp each other's fixtures
    /// — the failure mode that makes a suite look broken when it is only being
    /// run twice at once (trap D373B875).
    fn temp_root(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "materialbench_commit_{tag}_{}_{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A v1 factory patch (flicker-texture's rack corpus) as the v2 graph it converts to —
    /// these tests' fixture, the shape a Roll arrives in.
    fn graph_of(recipe: &TextureRecipe) -> MaterialGraph {
        MaterialGraph::from_recipe(recipe)
    }

    /// The slot `graph` is bound to. The CATALOG is the source of which slot a
    /// patch owns, so a test reads it off the document instead of restating it.
    fn slot_of(graph: &MaterialGraph) -> flicker_materials::MaterialId {
        graph
            .material
            .expect("every factory patch is bound to a slot")
    }

    /// The render class of the SHIPPED index row `slot` binds — what the bench hands
    /// [`commit`]. The catalog, not the test, says what a slot holds.
    fn class_at(slot: Option<flicker_materials::MaterialId>) -> Option<RenderClass> {
        let data = flicker_content::roots().data();
        let tables =
            flicker_materials::Tables::from_source(&flicker_materials::JsonTableSource::new(&data))
                .expect("the shipped material index loads");
        slot.and_then(|s| tables.render_class(s))
    }

    /// The committed document, read back through the gz seam by its logical path.
    fn doc_at(dir: &Path, seg: &str) -> String {
        flicker_content::package::read_text(&dir.join(format!("{seg}.texture.json")))
            .expect("the document reads back")
    }

    /// A commit writes the whole bundle in the layout `package/` uses, so a
    /// promotion is the same relative path under a different root — and that
    /// path is composed by the seam both sides share.
    #[test]
    fn a_commit_writes_the_maps_and_the_document_in_package_layout() {
        let root = temp_root("layout");
        let graph = graph_of(&presets::granite());
        let slot = slot_of(&graph);
        let out = commit(&graph, class_at(graph.material), 32, &root).expect("commit writes");

        assert_eq!(out.dir, flicker_content::bundle_dir(&root, slot));
        // And what that composes to, spelled out once: the zero-padded slot id.
        assert_eq!(out.dir, root.join("materials").join("010"));
        assert_eq!(out.files.len(), MapKind::ALL.len() + 1);
        for kind in MapKind::ALL {
            let png = out.dir.join(format!("010_{}.png", kind.role()));
            assert!(png.is_file(), "{kind:?} map missing at {}", png.display());
        }
        assert!(
            out.dir.join("010.texture.json.gz").is_file(),
            "the document's stem is the slot too, under the name the reader looks for"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// THE FORMAT: the bundle carries the v2 graph — `"version": 2`, nodes and an output
    /// — and the loader every reader uses (`load_material_json`) reads it back as the
    /// very graph that was committed.
    #[test]
    fn the_document_is_the_v2_graph_and_loads_back_exactly() {
        let root = temp_root("format");
        let graph = graph_of(&presets::basalt());
        let out = commit(&graph, class_at(graph.material), 16, &root).expect("commit writes");
        let text = doc_at(&out.dir, "011");
        let raw: serde_json::Value = serde_json::from_str(&text).expect("the document is JSON");
        assert_eq!(raw["version"], 2, "a v2 document says so");
        assert!(raw["nodes"].is_array() && raw["output"].is_object());
        assert_eq!(
            raw["material"], 11,
            "the slot rides where every reader looks for it"
        );
        assert_eq!(load_material_json(&text).expect("it loads"), graph);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A graph bound to nothing has no correct folder to land in — an id-keyed
    /// bundle needs a slot, and guessing a name is the failure this removes. It
    /// must be refused BEFORE the bake, having written nothing at all.
    #[test]
    fn an_unbound_graph_is_refused_before_it_costs_a_bake() {
        let root = temp_root("unbound");
        let graph = graph_of(&flicker_texture::random(0x5EED));
        assert!(graph.material.is_none(), "a rolled graph is scratch");

        let why = match commit(&graph, class_at(graph.material), 32, &root) {
            Err(CommitError::Refused(why)) => why,
            other => panic!("an unbound graph is a REFUSAL, not a failure: {other:?}"),
        };
        assert_eq!(why, Refusal::Unbound);
        assert!(
            why.token().starts_with('$'),
            "the refusal reaches the screen as a stringtable token: {}",
            why.token()
        );
        assert!(
            !root.exists(),
            "a refused commit writes nothing — not even the folder"
        );
    }

    /// THE SLOT LAW at the commit, through the ONE predicate and the bound row's class:
    /// Air's slot refuses everything, an emissive slot refuses a non-emissive row (and an
    /// unlisted one) — each a REFUSAL with a token, having written nothing — and an
    /// emissive slot ACCEPTS an emissive row, which lands in its own folder.
    #[test]
    fn the_slot_law_refuses_air_and_non_emissive_rows_in_the_emissive_slots() {
        use flicker_materials::EMISSIVE_FIRST;
        let root = temp_root("slotlaw");
        for (slot, class) in [
            (0, Some(RenderClass::HardEdge)),
            (0, Some(RenderClass::Emissive)),
            (EMISSIVE_FIRST, Some(RenderClass::HardEdge)),
            (EMISSIVE_FIRST, Some(RenderClass::Translucent)),
            (u8::MAX, Some(RenderClass::Blendable)),
            (u8::MAX, None),
        ] {
            let mut graph = graph_of(&presets::granite());
            graph.material = Some(slot);
            let why = match commit(&graph, class, 16, &root) {
                Err(CommitError::Refused(why)) => why,
                other => panic!("slot {slot} as {class:?} is a REFUSAL: {other:?}"),
            };
            assert_eq!(why, Refusal::SlotClosed, "slot {slot} as {class:?}");
            assert!(why.token().starts_with('$'), "slot {slot}: {}", why.token());
            assert!(
                !root.exists(),
                "slot {slot} as {class:?} wrote something it should have refused"
            );
        }

        // An emissive row in an emissive slot is what those slots are FOR.
        let mut glow = graph_of(&presets::granite());
        glow.material = Some(EMISSIVE_FIRST);
        let out = commit(&glow, Some(RenderClass::Emissive), 16, &root)
            .expect("an emissive row commits into an emissive slot");
        assert_eq!(out.dir, flicker_content::bundle_dir(&root, EMISSIVE_FIRST));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What a commit wrote opens again: [`graph_at`] reads the slot's document back through
    /// the one loader as the very graph committed; a slot with nothing there is `None`; a
    /// document that names another slot comes back bound to its FOLDER's; and a document
    /// that will not load is an error naming it, never a silent nothing.
    #[test]
    fn a_committed_bundle_opens_as_the_graph_that_was_committed() {
        let root = temp_root("graphat");
        let graph = graph_of(&presets::basalt());
        let slot = slot_of(&graph);
        assert_eq!(graph_at(&root, slot).expect("an empty tree reads"), None);
        commit(&graph, class_at(graph.material), 16, &root).expect("commit writes");
        assert_eq!(
            graph_at(&root, slot).expect("the bundle opens"),
            Some(graph.clone()),
            "the committed graph IS the one that opens"
        );
        assert_eq!(
            graph_at(&root, slot + 1).expect("reads"),
            None,
            "another slot"
        );

        // The folder decides the slot (the PackageBench's D-MISMATCH).
        let seg = flicker_materials::slot_dir(slot);
        let doc = flicker_content::bundle_dir(&root, slot).join(format!("{seg}.texture.json"));
        let mut elsewhere = graph.clone();
        elsewhere.material = Some(slot + 1);
        flicker_content::package::write_text(
            &doc,
            &serde_json::to_string_pretty(&elsewhere).unwrap(),
        )
        .unwrap();
        assert_eq!(
            graph_at(&root, slot)
                .expect("opens")
                .and_then(|g| g.material),
            Some(slot),
            "opened bound to its folder's slot"
        );

        flicker_content::package::write_text(&doc, "{\"nodes\": 7}").unwrap();
        let err = graph_at(&root, slot).expect_err("a broken document is an error");
        assert!(err.contains(&seg), "the error names the document: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// THE TWO TREES a committed look is found in: a scratch tree holding one bundle STAGED
    /// (slot A), one PROMOTED (slot B — a promotion moves it out of staging) and one in BOTH
    /// (slot C, where the staged — newer — one wins). `committed` opens each from where it
    /// is; a slot in neither tree is `None`; and a broken STAGED document is an error, never
    /// passed over for the promoted one beneath it.
    #[test]
    fn a_committed_look_opens_from_staging_first_then_package() {
        let tree = temp_root("twotrees");
        let (staging, package) = (tree.join("staging"), tree.join("package"));
        let staged = graph_of(&presets::granite());
        let promoted = graph_of(&presets::basalt());
        let (a, b) = (slot_of(&staged), slot_of(&promoted));
        commit(&staged, class_at(Some(a)), 16, &staging).expect("A stages");
        commit(&promoted, class_at(Some(b)), 16, &package).expect("B lands in package");
        let mut newer = graph_of(&presets::sandstone());
        let c = slot_of(&newer);
        commit(&newer, class_at(Some(c)), 16, &package).expect("C's older look is promoted");
        newer.seed ^= 0x5EED;
        commit(&newer, class_at(Some(c)), 16, &staging).expect("C's newer look is staged");

        let open = |slot| committed(&staging, &package, slot).expect("the trees read");
        assert_eq!(open(a), Some(staged), "a staged look opens from staging");
        assert_eq!(
            open(b),
            Some(promoted),
            "a promoted look opens from package"
        );
        assert_eq!(open(c), Some(newer), "staging — the newer commit — wins");
        assert_eq!(open(40), None, "a slot neither tree holds opens nothing");

        let seg = flicker_materials::slot_dir(c);
        flicker_content::package::write_text(
            &flicker_content::bundle_dir(&staging, c).join(format!("{seg}.texture.json")),
            "{\"nodes\": 7}",
        )
        .unwrap();
        assert!(
            committed(&staging, &package, c).is_err(),
            "a broken staged document is named, not skipped for the promoted one"
        );
        let _ = std::fs::remove_dir_all(&tree);
    }

    /// A list of authored graphs reads entry by entry through the one loader, `_comment`s
    /// beside it ignored; a list with no `graphs` array, or an entry that is not a material
    /// document, is an error that names it.
    #[test]
    fn an_authored_graph_list_reads_through_the_one_loader() {
        let a = graph_of(&presets::granite());
        let b = graph_of(&presets::hematite());
        let text = serde_json::json!({
            "_comment": "ignored",
            "graphs": [a, b],
        })
        .to_string();
        assert_eq!(
            graphs_in(&text).expect("the list reads"),
            vec![a.clone(), b]
        );
        assert!(graphs_in("{\"_comment\": \"no graphs\"}").is_err());
        let err = graphs_in(&serde_json::json!({"graphs": [a, {"nodes": 7}]}).to_string())
            .expect_err("a broken entry is an error");
        assert!(
            err.contains("graphs[1]"),
            "the error names the entry: {err}"
        );
    }

    /// A graph that cannot bake is refused with the GraphError that names the broken
    /// wire — and, because the bake runs before the folder is made, writes nothing.
    #[test]
    fn a_graph_that_cannot_bake_writes_nothing_and_says_why() {
        let root = temp_root("broken");
        let mut graph = graph_of(&presets::granite());
        graph.output.height = Some("no_such_node".into());
        let err = match commit(&graph, class_at(graph.material), 16, &root) {
            Err(CommitError::Graph(e)) => e,
            other => panic!("a broken graph is a graph error: {other:?}"),
        };
        assert!(
            err.to_string().contains("no_such_node"),
            "the error names the bad wire: {err}"
        );
        assert!(!root.exists(), "a graph that cannot bake writes nothing");
    }

    /// The slot IS the identity, so re-committing one replaces its bundle in
    /// place: the folder still holds exactly the seven maps and one document, and
    /// the document is the NEW one (D-OVERWRITE).
    #[test]
    fn a_second_commit_replaces_the_bundle_rather_than_accumulating() {
        let root = temp_root("overwrite");
        let first = graph_of(&presets::granite());
        let out = commit(&first, class_at(first.material), 32, &root).expect("first commit writes");

        let mut second = first.clone();
        second.seed ^= 0xABCD_1234;
        second.name = "Granite Rework".into();
        let again =
            commit(&second, class_at(second.material), 32, &root).expect("second commit writes");
        assert_eq!(again.dir, out.dir, "the same slot is the same folder");

        let mut pngs = 0;
        let mut docs = 0;
        for entry in std::fs::read_dir(&again.dir).expect("bundle folder reads") {
            let p = entry.expect("dir entry").path();
            match p.extension().and_then(|e| e.to_str()) {
                Some("png") => pngs += 1,
                Some("gz") => docs += 1,
                other => panic!("unexpected file {p:?} ({other:?})"),
            }
        }
        assert_eq!(pngs, MapKind::ALL.len(), "seven maps, not fourteen");
        assert_eq!(docs, 1, "one document");
        let back = load_material_json(&doc_at(&again.dir, "010")).expect("it loads");
        assert_eq!(back, second, "the bundle carries the SECOND graph");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// PNGs land RAW and the document lands GZ — the at-rest split that makes a
    /// promotion a byte move instead of a transcode.
    #[test]
    fn maps_stay_raw_and_the_document_lands_gz() {
        let root = temp_root("atrest");
        let graph = graph_of(&presets::basalt());
        assert_eq!(slot_of(&graph), 11, "basalt is slot 11");
        let out = commit(&graph, class_at(graph.material), 32, &root).expect("commit writes");

        let png = out.dir.join("011_BaseColor.png");
        assert!(png.is_file(), "the map is at its plain path");
        assert!(
            !out.dir.join("011_BaseColor.png.gz").exists(),
            "a PNG must not be gzipped"
        );
        let bytes = std::fs::read(&png).expect("read map");
        assert_eq!(
            &bytes[..4],
            b"\x89PNG",
            "the map is a real PNG, not a gz stream"
        );

        let plain = out.dir.join("011.texture.json");
        assert!(
            plain.with_extension("json.gz").is_file(),
            "the document is gz at rest"
        );
        assert!(!plain.is_file(), "no stale raw twin beside the gz");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// THE ROUND TRIP: the committed document reads back through the loader the
    /// Content Manager and the engine use and RE-BAKES at the rung to exactly the bytes
    /// written — the reason the document is the artifact and the maps its output.
    #[test]
    fn a_committed_bundle_loads_back_and_rebakes_to_the_written_maps() {
        let root = temp_root("roundtrip");
        let graph = graph_of(&presets::sandstone());
        assert_eq!(slot_of(&graph), 12, "sandstone is slot 12");
        let rung = 32;
        let out = commit(&graph, class_at(graph.material), rung, &root).expect("commit writes");

        let back = load_material_json(&doc_at(&out.dir, "012")).expect("the document loads");
        assert_eq!(back, graph, "the graph survives the trip");
        let rebaked = bake_graph(&back, rung).expect("the loaded graph bakes");
        for kind in MapKind::ALL {
            let written = image::open(out.dir.join(format!("012_{}.png", kind.role())))
                .expect("the written map decodes")
                .to_rgba8();
            let want = rebaked.get(kind).expect("every map bakes");
            assert_eq!(written.width(), rung, "{kind:?} was written at the rung");
            assert_eq!(
                written.as_raw(),
                &want.pixels,
                "{kind:?}: the re-bake is the bytes that were written"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Everything a commit writes must classify for the Content Manager's Type
    /// column — an artifact that lands as `Unknown` is one a reviewer cannot
    /// reason about.
    #[test]
    fn everything_committed_classifies() {
        let root = temp_root("classify");
        let graph = graph_of(&presets::hematite());
        assert_eq!(slot_of(&graph), 40, "hematite is slot 40");
        let out = commit(&graph, class_at(graph.material), 32, &root).expect("commit writes");
        for f in &out.files {
            let class = flicker_content::classify_package(f);
            assert_ne!(
                class,
                PackageClass::Unknown,
                "{} classified Unknown",
                f.display()
            );
        }
        assert_eq!(
            flicker_content::classify_package(&out.dir.join("040.texture.json")),
            PackageClass::TextureRecipe
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Nothing here may write into `package/`. Promotion is the Content Manager's
    /// job, and a bench that reached past staging would silently undo the review
    /// step the whole tier exists for.
    #[test]
    fn a_commit_touches_only_the_staging_root_it_was_given() {
        let root = temp_root("scope");
        let graph = graph_of(&presets::granite());
        let out = commit(&graph, class_at(graph.material), 16, &root).expect("commit writes");
        for f in &out.files {
            assert!(
                f.starts_with(&root),
                "{} escaped the staging root",
                f.display()
            );
        }
        assert!(
            !root.join("package").exists(),
            "a bench never writes into package/"
        );
    }

    /// Each map ships in the narrowest form that carries its meaning, and must
    /// decode back to exactly the values the baker produced. Getting this wrong
    /// would be invisible in a file listing and wrong in every surface.
    #[test]
    fn maps_encode_narrow_and_decode_back_to_the_baked_values() {
        let root = temp_root("channels");
        let graph = graph_of(&presets::granite());
        let out = commit(&graph, class_at(graph.material), 32, &root).expect("commit writes");
        let set = bake_graph(&graph, 32).expect("granite bakes");

        for kind in MapKind::ALL {
            let path = out.dir.join(format!("010_{}.png", kind.role()));
            let img = image::open(&path).expect("map decodes");
            // COLOUR maps (base colour, emit) and `Normal` keep three channels; a
            // colour shipped as L8 would lose its hue entirely.
            let expect_color = if kind.is_color() || kind == MapKind::Normal {
                image::ColorType::Rgb8
            } else {
                image::ColorType::L8
            };
            assert_eq!(
                img.color(),
                expect_color,
                "{kind:?} shipped in the wrong form"
            );

            // Widened back, every channel that carried meaning must be intact.
            let got = img.to_rgba8();
            let want = set.get(kind).unwrap();
            for (i, (g, w)) in got
                .as_chunks::<4>()
                .0
                .iter()
                .zip(want.pixels.as_chunks::<4>().0.iter())
                .enumerate()
            {
                assert_eq!(g[0], w[0], "{kind:?} texel {i} red changed");
                if expect_color == image::ColorType::Rgb8 {
                    assert_eq!(
                        (g[1], g[2]),
                        (w[1], w[2]),
                        "{kind:?} texel {i} colour changed"
                    );
                }
                assert_eq!(g[3], 255, "{kind:?} texel {i} lost its opaque alpha");
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
