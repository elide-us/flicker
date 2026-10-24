//! What a [`MaterialSample`](crate::MaterialSample) wears: [`SampleMaps`], and the three
//! ways a host gets them.
//!
//! - **A live map set** — textures the host already holds, in [`MapKind::ALL`] order (the
//!   MaterialBench's preview set): [`SampleMaps::of_set`] borrows them, uploading nothing.
//! - **A promoted bundle** — [`decode_bundle_maps`] reads its seven PNGs off disk into RGBA8
//!   (pure CPU, worker-safe), and [`upload_decoded`] makes textures of them.
//! - **A material with no bundle** — [`FlatMaterial::of`] reads a flat PBR off its index
//!   row, and a [`FlatTextures`] cache turns it into 1×1 textures, sharing identical ones.
//!
//! What this crate UPLOADS it owns, and gives back exactly once on a consuming `release`:
//! an [`OwnedMaps`] from [`upload_decoded`], a [`FlatTextures`] cache with everything it
//! made. A borrowed set ([`SampleMaps::of_set`]) is the host's and is never freed here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flicker::render::{PbrMaps, Renderer, TextureHandle};
use flicker_content::MaterialBundle;
use flicker_materials::{MaterialDef, RenderClass};
use flicker_texture::MapKind;

/// The maps a sample is drawn in: the albedo, the PBR set, and the draw-time emissive gain
/// (the multiplier on the Emit map — `1.0` is the texel as authored, above it a glow is HDR
/// radiance the stage's `bloom` spreads; see `MeshDrawOptions::emissive_gain`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleMaps {
    pub albedo: TextureHandle,
    pub pbr: PbrMaps,
    pub emissive_gain: f32,
}

impl SampleMaps {
    /// A map set's textures in [`MapKind::ALL`] order (a `MapSet` on the GPU — the
    /// MaterialBench's seven preview textures), borrowed as they are: nothing is uploaded,
    /// and nothing here can free them. `None` when the set holds no base colour (not
    /// created yet).
    pub fn of_set(set: &[TextureHandle], emissive_gain: f32) -> Option<Self> {
        let at = |kind: MapKind| set.get(set_index(kind)).copied();
        Some(Self {
            albedo: at(MapKind::BaseColor)?,
            pbr: pbr_maps(at),
            emissive_gain,
        })
    }

    /// Every texture this set binds — the albedo and each bound PBR map.
    fn textures(&self) -> impl Iterator<Item = TextureHandle> {
        let p = self.pbr;
        std::iter::once(self.albedo).chain(
            [p.normal, p.roughness, p.metalness, p.ao, p.emit]
                .into_iter()
                .flatten(),
        )
    }
}

/// Where `kind` sits in a map set: its position in [`MapKind::ALL`].
fn set_index(kind: MapKind) -> usize {
    MapKind::ALL
        .iter()
        .position(|&k| k == kind)
        .unwrap_or_default()
}

/// THE binding, as data: the map kind each PBR slot reads, in [`PbrMaps`] field order
/// (normal, roughness, metalness, AO, Emit). `Height` is in no slot — it is terrain
/// displacement data, not a shading input. `Emit` IS: the whole point of a glow is that the
/// lit sample shows it.
const PBR_KINDS: [MapKind; 5] = [
    MapKind::Normal,
    MapKind::Roughness,
    MapKind::Metallic,
    MapKind::Ao,
    MapKind::Emit,
];

/// A [`PbrMaps`] asking `map` for each slot's kind ([`PBR_KINDS`]). Every constructor here
/// goes through it, so the three can never bind differently.
fn pbr_maps(map: impl FnMut(MapKind) -> Option<TextureHandle>) -> PbrMaps {
    let [normal, roughness, metalness, ao, emit] = PBR_KINDS.map(map);
    PbrMaps {
        normal,
        roughness,
        metalness,
        ao,
        emit,
    }
}

/// Sample maps THIS crate uploaded ([`upload_decoded`]): their textures are the owner's to
/// give back, exactly once, with [`OwnedMaps::release`]. Derefs to the [`SampleMaps`] a
/// sample draws with. Deliberately not `Clone`/`Copy` — one owner, one free.
#[must_use = "an uploaded map set holds its textures until it is released"]
pub struct OwnedMaps(SampleMaps);

impl std::ops::Deref for OwnedMaps {
    type Target = SampleMaps;

    fn deref(&self) -> &SampleMaps {
        &self.0
    }
}

impl OwnedMaps {
    /// Free every texture the upload made. Consumes the set: a freed set cannot be drawn
    /// or freed again — the renderer reuses a freed slot, so a second free would take
    /// whatever was uploaded into it since.
    pub fn release(self, r: &mut Renderer) {
        for t in self.0.textures() {
            r.free_texture(t);
        }
    }
}

/// A material's flat PBR — the stand-in a sample wears for a material that has no bundle:
/// its index colour over one roughness and metalness for the whole surface, and a glow of
/// its own colour if it is an emissive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlatMaterial {
    /// Base colour, `0..=1` per channel in the index's own domain (the bytes a swatch of it
    /// shows, uploaded sRGB).
    pub color: [f32; 3],
    pub roughness: f32,
    pub metalness: f32,
    /// The Emit colour, for an emissive-class material; `None` emits nothing.
    pub emissive: Option<[f32; 3]>,
}

impl FlatMaterial {
    /// Read a flat PBR off an index row. The finish is a per-family default — the index
    /// carries a colour and a render class, not a finish:
    ///
    /// | row | metalness | roughness |
    /// |---|---|---|
    /// | category `metal` / `alloy` | 1 | 0.35 |
    /// | render class `translucent` (gems, liquids, glass) | 0 | 0.12 |
    /// | category `liquid` (not translucent: lava) | 0 | 0.15 |
    /// | category `rock` / `ore` / `mineral` | 0 | 0.7 |
    /// | everything else | 0 | 0.8 |
    ///
    /// First match wins, top down; "everything else" is soil, sediment, surface, wood,
    /// plant, organic, crafted, lore, solid and void. An `emissive`-class row glows in its
    /// own colour.
    pub fn of(def: &MaterialDef) -> FlatMaterial {
        let class = def.render_class;
        let (metalness, roughness) = match def.category.as_str() {
            "metal" | "alloy" => (1.0, 0.35),
            _ if class == Some(RenderClass::Translucent) => (0.0, 0.12),
            "liquid" => (0.0, 0.15),
            "rock" | "ore" | "mineral" => (0.0, 0.7),
            _ => (0.0, 0.8),
        };
        FlatMaterial {
            color: def.color,
            roughness,
            metalness,
            emissive: (class == Some(RenderClass::Emissive)).then_some(def.color),
        }
    }

    /// The one texel this material shows as `kind` — the key a [`FlatTextures`] cache
    /// shares textures by. Colour maps (`MapKind::is_color`) are sRGB, data maps linear.
    pub fn texel(&self, kind: MapKind) -> Texel {
        let rgb = |c: [f32; 3]| [byte(c[0]), byte(c[1]), byte(c[2]), 255];
        let grey = |v: f32| {
            let b = byte(v);
            [b, b, b, 255]
        };
        let rgba = match kind {
            MapKind::BaseColor => rgb(self.color),
            // The flat tangent-space normal.
            MapKind::Normal => [128, 128, 255, 255],
            MapKind::Roughness => grey(self.roughness),
            MapKind::Metallic => grey(self.metalness),
            // Unoccluded.
            MapKind::Ao => [255; 4],
            // A flat surface has no relief: a level field at mid height. (No shading slot
            // reads Height, so a sample never asks for it.)
            MapKind::Height => [128, 128, 128, 255],
            MapKind::Emit => self.emissive.map_or([0, 0, 0, 255], rgb),
        };
        Texel {
            srgb: kind.is_color(),
            rgba,
        }
    }
}

/// A `0..=1` channel as a byte.
fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// One 1×1 texture's content: its bytes AND its colour space. The space is part of the
/// identity — the same bytes sampled sRGB and sampled linear are different textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Texel {
    /// Colour data (albedo, Emit — uploaded sRGB) rather than linear data.
    pub srgb: bool,
    pub rgba: [u8; 4],
}

/// Flat materials as 1×1 textures, each distinct [`Texel`] made ONCE and shared by every
/// material that shows it — a whole index of flat materials needs one flat normal, one AO,
/// two metalnesses and a handful of roughnesses, not one of each per material.
///
/// The cache OWNS what it made: own ONE cache for as long as any maps it handed out are
/// drawn (a rebuilt cache re-uploads every texel it had already made), and give the
/// textures back once with [`FlatTextures::release`] when the last of them is done.
#[derive(Default)]
pub struct FlatTextures {
    made: HashMap<Texel, TextureHandle>,
}

impl FlatTextures {
    pub fn new() -> Self {
        Self::default()
    }

    /// `flat` as sample maps, making only the texels this cache has not made yet. The maps
    /// borrow the cache's textures: they are valid until [`Self::release`].
    pub fn maps_for(
        &mut self,
        r: &mut Renderer,
        flat: &FlatMaterial,
        emissive_gain: f32,
    ) -> SampleMaps {
        let mut texture = |kind: MapKind| {
            let t = flat.texel(kind);
            *self.made.entry(t).or_insert_with(|| {
                if t.srgb {
                    r.load_texture(&t.rgba, 1, 1)
                } else {
                    r.load_texture_linear(&t.rgba, 1, 1)
                }
            })
        };
        let albedo = texture(MapKind::BaseColor);
        SampleMaps {
            albedo,
            pbr: pbr_maps(|kind| Some(texture(kind))),
            emissive_gain,
        }
    }

    /// How many distinct textures this cache has made.
    pub fn len(&self) -> usize {
        self.made.len()
    }

    pub fn is_empty(&self) -> bool {
        self.made.is_empty()
    }

    /// Free every texture this cache made (scene `exit`). Consumes the cache, so each is
    /// freed exactly once; every [`SampleMaps`] it handed out is dead after this — release
    /// it only once nothing draws them.
    pub fn release(self, r: &mut Renderer) {
        for t in self.made.into_values() {
            r.free_texture(t);
        }
    }
}

/// A bundle's maps decoded to RGBA8 at `px × px`, one buffer per [`MapKind::ALL`] entry —
/// the CPU half of drawing a promoted material, safe to produce on a worker. `Height` is
/// required on disk but NOT decoded: no shading slot reads it, so its buffer is empty.
pub struct DecodedMaps {
    px: u32,
    maps: [Vec<u8>; MapKind::ALL.len()],
}

impl DecodedMaps {
    /// The edge length every map was decoded to.
    pub fn px(&self) -> u32 {
        self.px
    }

    /// `kind`'s pixels, `px × px × 4` bytes — EMPTY for `Height`, which is never decoded.
    pub fn get(&self, kind: MapKind) -> &[u8] {
        &self.maps[set_index(kind)]
    }
}

/// The bundle's `kind` map — found by its content-standard `_<role>.png` suffix (the one
/// naming the commit writes). `None` when the bundle has no such map.
pub fn map_path(bundle: &MaterialBundle, kind: MapKind) -> Option<PathBuf> {
    let suffix = format!("_{}.png", kind.role());
    bundle
        .maps
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&suffix))
        })
        .cloned()
}

/// Decode an image file and resize it to exactly `w × h` RGBA8. `None` (with a warn) for a
/// file that will not decode, so a caller keeps what it had — visibly not the map, rather
/// than silently a blank.
pub fn decode_map(path: &Path, w: u32, h: u32) -> Option<Vec<u8>> {
    match image::open(path) {
        Ok(img) => Some(img.thumbnail_exact(w, h).into_rgba8().into_raw()),
        Err(e) => {
            tracing::warn!("material map {} did not decode: {e}", path.display());
            None
        }
    }
}

/// Decode `bundle`'s maps at `px × px`. Pure CPU — run it on a worker. All SEVEN must be on
/// disk, but `Height` is not decoded (no shading slot reads it; its buffer stays empty).
/// `None`, with a warn naming what is missing, for a bundle short of any map or a map that
/// will not decode: a sample is drawn in a whole material or not at all. Nothing cancels
/// it — a host whose sample can close mid-decode uses [`decode_bundle_maps_with`].
pub fn decode_bundle_maps(bundle: &MaterialBundle, px: u32) -> Option<DecodedMaps> {
    decode_bundle_maps_with(bundle, px, &|| false)
}

/// [`decode_bundle_maps`] that its host can CANCEL. `cancelled` is asked BETWEEN maps —
/// before each one, the first included — and the first `true` ends the decode there, after
/// the map in hand: `None`, quietly (a cancel is the host's choice, not a fault). A host
/// that closes while a decode runs (a modal dismissed mid-decode) passes its flag here, so
/// the worker it shares is free again after at most ONE map's decode rather than the whole
/// bundle's — a 4K map is tens of milliseconds, a 4K bundle well over a hundred.
pub fn decode_bundle_maps_with(
    bundle: &MaterialBundle,
    px: u32,
    cancelled: &dyn Fn() -> bool,
) -> Option<DecodedMaps> {
    if px == 0 {
        tracing::warn!(
            "material bundle {}: asked to decode at 0 px — nothing to draw",
            bundle.dir.display()
        );
        return None;
    }
    let mut maps: [Vec<u8>; MapKind::ALL.len()] = std::array::from_fn(|_| Vec::new());
    for (slot, kind) in maps.iter_mut().zip(MapKind::ALL) {
        if cancelled() {
            tracing::debug!(
                "material bundle {}: decode cancelled before its {} map",
                bundle.dir.display(),
                kind.role()
            );
            return None;
        }
        let Some(path) = map_path(bundle, kind) else {
            tracing::warn!(
                "material bundle {} has no {} map — it cannot be drawn as a sample",
                bundle.dir.display(),
                kind.role()
            );
            return None;
        };
        if kind != MapKind::Height {
            *slot = decode_map(&path, px, px)?;
        }
    }
    Some(DecodedMaps { px, maps })
}

/// Upload decoded maps as sample maps this crate OWNS: colour maps (`MapKind::is_color` —
/// albedo, Emit) sRGB, data maps linear. `Height` is not uploaded: no shading slot reads it.
/// Give the textures back with [`OwnedMaps::release`] once nothing draws them — upload a
/// material once and keep its maps, rather than re-uploading it.
pub fn upload_decoded(r: &mut Renderer, maps: &DecodedMaps, emissive_gain: f32) -> OwnedMaps {
    let n = maps.px;
    let mut upload = |kind: MapKind| {
        let pixels = maps.get(kind);
        if kind.is_color() {
            r.load_texture(pixels, n, n)
        } else {
            r.load_texture_linear(pixels, n, n)
        }
    };
    let albedo = upload(MapKind::BaseColor);
    OwnedMaps(SampleMaps {
        albedo,
        pbr: pbr_maps(|kind| Some(upload(kind))),
        emissive_gain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use std::collections::HashSet;

    /// The shipped material index — the rows the catalog reads.
    fn shipped_rows() -> Vec<MaterialDef> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/data");
        Tables::from_source(&JsonTableSource::new(dir))
            .expect("the shipped material tables load")
            .materials()
            .to_vec()
    }

    /// An index row with only what [`FlatMaterial::of`] reads set.
    fn row(category: &str, class: &str, color: [f32; 3]) -> MaterialDef {
        serde_json::from_value(serde_json::json!({
            "id": 7, "name": "Probe", "category": category, "render_class": class,
            "hardness": 1.0, "brittleness": 0.5, "water_capacity": 0.0, "viscosity": 1.0,
            "density_g_cm3": 1.0, "color": color
        }))
        .expect("a well-formed row")
    }

    /// A metal, an emissive, a translucent and a rock row of the SHIPPED index each read as
    /// their family's finish — picked by family, not by name, so a renamed row still tests.
    #[test]
    fn shipped_rows_read_as_their_familys_finish() {
        let rows = shipped_rows();
        let find = |what: &str, pick: &dyn Fn(&MaterialDef) -> bool| {
            rows.iter()
                .find(|&m| pick(m))
                .cloned()
                .unwrap_or_else(|| panic!("the shipped index has no {what} row"))
        };

        let metal = find("metal", &|m| m.category == "metal");
        let f = FlatMaterial::of(&metal);
        assert_eq!((f.metalness, f.roughness), (1.0, 0.35), "{}", metal.name);
        assert_eq!(f.color, metal.color, "the index colour is the albedo");
        assert_eq!(f.emissive, None, "{} does not glow", metal.name);

        let glow = find("emissive", &|m| {
            m.render_class == Some(RenderClass::Emissive)
        });
        let f = FlatMaterial::of(&glow);
        assert_eq!(
            f.emissive,
            Some(glow.color),
            "{} glows in its own colour",
            glow.name
        );
        assert_eq!(f.metalness, 0.0);

        let clear = find("translucent", &|m| {
            m.render_class == Some(RenderClass::Translucent)
        });
        let f = FlatMaterial::of(&clear);
        assert_eq!((f.metalness, f.roughness), (0.0, 0.12), "{}", clear.name);
        assert_eq!(f.emissive, None);

        let rock = find("rock", &|m| m.category == "rock");
        let f = FlatMaterial::of(&rock);
        assert_eq!((f.metalness, f.roughness), (0.0, 0.7), "{}", rock.name);
        assert_eq!(f.emissive, None);

        // Every shipped row lands on one of the table's finishes, and glows exactly when
        // it is an emissive.
        for m in &rows {
            let f = FlatMaterial::of(m);
            assert!(
                [0.12, 0.15, 0.35, 0.7, 0.8].contains(&f.roughness)
                    && [0.0, 1.0].contains(&f.metalness),
                "{}: {f:?}",
                m.name
            );
            assert_eq!(
                f.emissive.is_some(),
                m.render_class == Some(RenderClass::Emissive),
                "{}",
                m.name
            );
        }
    }

    /// The table is read top-down, first match wins: an alloy is a metal; a translucent
    /// mineral is glassy, not stony; a liquid is glassy when translucent and wet when not
    /// (lava); anything unlisted is the matte default.
    #[test]
    fn the_finish_table_reads_first_match_down() {
        let c = [0.5, 0.4, 0.3];
        let finish = |cat: &str, class: &str| {
            let f = FlatMaterial::of(&row(cat, class, c));
            (f.metalness, f.roughness)
        };
        assert_eq!(finish("alloy", "hard_edge"), (1.0, 0.35));
        assert_eq!(finish("mineral", "translucent"), (0.0, 0.12));
        assert_eq!(finish("mineral", "hard_edge"), (0.0, 0.7));
        assert_eq!(finish("ore", "hard_edge"), (0.0, 0.7));
        assert_eq!(finish("liquid", "translucent"), (0.0, 0.12));
        assert_eq!(finish("liquid", "emissive"), (0.0, 0.15));
        for cat in [
            "soil", "sediment", "surface", "wood", "plant", "organic", "crafted",
        ] {
            assert_eq!(finish(cat, "blendable"), (0.0, 0.8), "{cat}");
        }
        assert_eq!(
            FlatMaterial::of(&row("liquid", "emissive", c)).emissive,
            Some(c)
        );
    }

    /// THE CACHE KEY: identical texels share one texture and different ones never do — the
    /// colour space included, so a white sRGB albedo and a white linear AO are two textures.
    /// Over the whole shipped index the shared maps collapse to a handful of textures.
    #[test]
    fn flat_texels_dedup_by_bytes_and_colour_space() {
        let a = FlatMaterial::of(&row("rock", "hard_edge", [0.5, 0.4, 0.3]));
        let b = FlatMaterial::of(&row("ore", "hard_edge", [0.5, 0.4, 0.3]));
        let chrome = FlatMaterial::of(&row("metal", "hard_edge", [1.0, 1.0, 1.0]));
        for kind in [
            MapKind::BaseColor,
            MapKind::Normal,
            MapKind::Roughness,
            MapKind::Metallic,
            MapKind::Ao,
            MapKind::Emit,
        ] {
            assert_eq!(a.texel(kind), b.texel(kind), "{kind:?} is one texture");
        }
        assert_ne!(a.texel(MapKind::Metallic), chrome.texel(MapKind::Metallic));
        assert_ne!(
            a.texel(MapKind::Roughness),
            chrome.texel(MapKind::Roughness)
        );

        // Same bytes, different space: never one texture.
        let white_albedo = chrome.texel(MapKind::BaseColor);
        let white_ao = chrome.texel(MapKind::Ao);
        assert_eq!(white_albedo.rgba, white_ao.rgba);
        assert!(white_albedo.srgb && !white_ao.srgb);
        assert_ne!(white_albedo, white_ao);

        // No glow is the black sRGB colour; a glow is its own colour, sRGB.
        assert_eq!(
            a.texel(MapKind::Emit),
            Texel {
                srgb: true,
                rgba: [0, 0, 0, 255]
            }
        );
        let lava = FlatMaterial::of(&row("liquid", "emissive", [1.0, 0.5, 0.0]));
        assert_eq!(
            lava.texel(MapKind::Emit),
            Texel {
                srgb: true,
                rgba: [255, 128, 0, 255]
            }
        );

        // The shipped index: one flat normal, one AO, two metalnesses, the table's five
        // roughnesses at most — every material's shared maps fit in a handful of textures.
        let rows = shipped_rows();
        let distinct = |kind: MapKind| {
            rows.iter()
                .map(|m| FlatMaterial::of(m).texel(kind))
                .collect::<HashSet<_>>()
                .len()
        };
        assert_eq!(distinct(MapKind::Normal), 1);
        assert_eq!(distinct(MapKind::Ao), 1);
        assert_eq!(distinct(MapKind::Metallic), 2);
        assert!(distinct(MapKind::Roughness) <= 5);
    }

    /// THE BINDING `of_set` reads: a set's textures sit in [`MapKind::ALL`] order, so the
    /// albedo is index 0 and the five PBR slots — normal, roughness, metalness, AO, Emit,
    /// in `PbrMaps` field order — are indices 1, 2, 3, 4 and 6. Index 5 (Height) is bound
    /// nowhere. Tested through the index table `of_set` reads: a `TextureHandle` cannot be
    /// made outside the renderer, so the table is the testable half of the binding.
    #[test]
    fn of_set_binds_albedo_0_the_pbr_slots_1_to_4_and_6_never_5() {
        assert_eq!(set_index(MapKind::BaseColor), 0, "the albedo is index 0");
        assert_eq!(
            PBR_KINDS.map(set_index),
            [1, 2, 3, 4, 6],
            "normal, roughness, metalness, AO, Emit"
        );
        assert_eq!(set_index(MapKind::Height), 5);
        assert!(
            !PBR_KINDS.contains(&MapKind::Height),
            "Height (index 5) feeds no shading slot"
        );
        // Every bound kind finds its own position — no two slots read one texture.
        let mut bound: Vec<usize> = PBR_KINDS.map(set_index).to_vec();
        bound.push(set_index(MapKind::BaseColor));
        bound.sort_unstable();
        bound.dedup();
        assert_eq!(bound.len(), 6, "six distinct textures are bound");
    }

    /// A pid+counter scratch package root, so parallel tests never share a folder.
    fn scratch(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "flicker_materialview_{name}_{}_{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A real bundle in a scratch package root: seven 8×8 solid PNGs, each its own colour,
    /// and the recipe — the one layout `bundle_dir` spells, found by the P3 predicate.
    fn scratch_bundle(name: &str, colours: [[u8; 3]; 7]) -> (PathBuf, MaterialBundle) {
        let root = scratch(name);
        let slot = 12;
        let dir = flicker_content::bundle_dir(&root, slot);
        std::fs::create_dir_all(&dir).unwrap();
        let seg = flicker_materials::slot_dir(slot);
        for (kind, rgb) in MapKind::ALL.into_iter().zip(colours) {
            image::RgbImage::from_pixel(8, 8, image::Rgb(rgb))
                .save(dir.join(format!("{seg}_{}.png", kind.role())))
                .expect("a map encodes");
        }
        flicker_content::package::write_text(&dir.join(format!("{seg}.texture.json")), "{}")
            .expect("the recipe writes");
        let bundle = flicker_content::bundle_at(&root, slot).expect("the P3 predicate sees it");
        (root, bundle)
    }

    /// A bundle's PNGs decode to `px²` RGBA8 buffers, each map under its own kind — a map
    /// read into the wrong slot would shade with, say, the roughness as albedo. `Height` must
    /// be on disk but is never decoded (no shading slot reads it): its buffer is empty.
    #[test]
    fn a_bundle_decodes_to_its_seven_maps_in_kind_order() {
        let colours = [
            [200, 90, 30],
            [128, 128, 255],
            [60, 60, 60],
            [250, 250, 250],
            [180, 180, 180],
            [10, 20, 30],
            [255, 140, 0],
        ];
        let (root, bundle) = scratch_bundle("decode", colours);
        assert_eq!(
            map_path(&bundle, MapKind::BaseColor)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
            Some(format!(
                "{}_BaseColor.png",
                flicker_materials::slot_dir(bundle.slot)
            ))
        );
        let maps = decode_bundle_maps(&bundle, 4).expect("a whole bundle decodes");
        assert_eq!(maps.px(), 4);
        for (kind, [r, g, b]) in MapKind::ALL.into_iter().zip(colours) {
            let px = maps.get(kind);
            if kind == MapKind::Height {
                assert!(px.is_empty(), "Height is required on disk, never decoded");
                continue;
            }
            assert_eq!(px.len(), 4 * 4 * 4, "{kind:?} is px² RGBA8");
            assert_eq!(&px[..4], &[r, g, b, 255], "{kind:?} is its own map");
        }
        assert!(
            decode_bundle_maps(&bundle, 0).is_none(),
            "0 px is nothing to draw"
        );

        // A bundle short of ANY map is not drawn at all — never drawn half-dressed. Height
        // included: it is not decoded, but a bundle without it is not a whole bundle.
        for missing in ["_Emit.png", "_Height.png"] {
            let mut short = MaterialBundle {
                slot: bundle.slot,
                dir: bundle.dir.clone(),
                recipe: bundle.recipe.clone(),
                maps: bundle.maps.clone(),
            };
            short
                .maps
                .retain(|p| !p.to_string_lossy().ends_with(missing));
            assert_eq!(short.maps.len(), 6);
            assert!(
                decode_bundle_maps(&short, 4).is_none(),
                "no {missing}, no sample"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A decode its host CANCELS stops between maps: the hook is asked before each map, and
    /// the first `true` ends the decode after the map in hand — `None`, and no further map
    /// is touched. Never cancelled, it is asked once per map and the bundle decodes whole.
    #[test]
    fn a_cancelled_decode_stops_after_the_map_in_hand() {
        use std::cell::Cell;
        let (root, bundle) = scratch_bundle("cancel", [[90, 60, 30]; 7]);
        let asked = &Cell::new(0usize);
        let count = |cancel_at: usize| {
            move || {
                asked.set(asked.get() + 1);
                asked.get() >= cancel_at
            }
        };

        let whole = decode_bundle_maps_with(&bundle, 4, &count(usize::MAX));
        assert!(whole.is_some(), "never cancelled, the bundle decodes whole");
        assert_eq!(asked.get(), MapKind::ALL.len(), "asked once per map");

        // Cancelled once two maps are in hand: asked a third time, then nothing after.
        asked.set(0);
        assert!(decode_bundle_maps_with(&bundle, 4, &count(3)).is_none());
        assert_eq!(asked.get(), 3, "the decode stopped at the first `true`");

        // Cancelled before it starts: no map is decoded at all.
        asked.set(0);
        assert!(decode_bundle_maps_with(&bundle, 4, &count(1)).is_none());
        assert_eq!(asked.get(), 1);

        // The plain decode is the never-cancelled one.
        assert_eq!(
            decode_bundle_maps(&bundle, 4).map(|m| m.px()),
            whole.map(|m| m.px())
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
