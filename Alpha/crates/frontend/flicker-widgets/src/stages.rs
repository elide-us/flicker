//! **The ONE stage compiler** — `stages.<source>` JSON → [`StageDef`].
//!
//! A nested `surface` node names a stage SOURCE; this module turns the authored block
//! (lighting preset · clear · camera · layers · attachments · passes · rate) into the
//! typed definition every filler consumes. It replaced three private parsers (the loomforge doll rig, the globe, the
//! Sablework lit sample) that each knew a third of the vocabulary and rejected the rest —
//! a `graticule` in a doll stage or a `ring` on a globe was a warning in one reader and
//! silence in another, and the globe's copy dropped the sky palette on the floor. One
//! vocabulary, one reader, one set of defaults.
//!
//! **Fail loud (rule 4BB12A75).** A name that resolves to nothing is reported, never
//! swallowed. [`compile_stage`] returns every PROBLEM it found as data — an unknown `draw`
//! kind, an unauthored lighting preset, a key no stage has, a colour whose `$token` never
//! resolved — so the shipped content is GATED at build time
//! (`every_shipped_stage_compiles_clean`) and the runtime path ([`stage_def`]) warns with
//! the same words. A value that fails still degrades to its default: a malformed style file
//! costs the authored look, never the picture.
//!
//! **The recipe.** `passes` is the ordered list of engine passes around the content — the
//! sky behind it, the volumetric disk and the ground fog over it — read here into
//! [`PassDef`]s over the surface's [`Attachments`]. Numbers a SIMULATION owns are not
//! authored: a `*_bind` names the key the scene publishes each frame
//! ([`StageInputs`](flicker_render::StageInputs)), and the bind REPLACES the authored
//! field at apply time — so the whole recipe compiles once, at load. Draw ORDER is never
//! authored: it is derived from what each pass reads and writes
//! ([`StageDef::pass_order`]).
//!
//! Colours arrive as four floats: [`load_styles`](crate::load_styles) has already resolved
//! `$token` refs against the one palette. A `$name` still present here is a token the
//! palette does not define — reported.

use std::collections::HashMap;

use flicker_render::{
    Attachment, AttachmentFormat, Attachments, BloomPass, CompositePass, Driver, DriverKind,
    FogSlot, GroundFog, GroundFogPass, Light, LightKind, LightRig, PassDef, PassKind, Rate,
    ShadowMapPass, StageCamera, StageDef, StageLayer, TonemapGradePass, TonemapSlot, Vec2, Vec3,
    VolumetricDisk, VolumetricPass, VolumetricSlot, Water, WaterPass, WaterSlot, WaveKind,
    WaveSource, MAX_LIGHTS, MAX_WAVE_SOURCES,
};
use serde_json::Value as Json;

/// Compile the named source, warning each problem. `None` when `stages.<source>` is
/// not authored at all (also warned) — the caller decides what an absent stage costs.
pub fn stage_def(styles: &Json, source: &str) -> Option<StageDef> {
    match compile_stage(styles, source) {
        Some((def, problems)) => {
            for p in &problems {
                tracing::warn!("{p}");
            }
            Some(def)
        }
        None => {
            tracing::warn!(
                "stages.{source} is not authored — nothing says what that surface renders"
            );
            None
        }
    }
}

/// Every authored source, keyed by name — the preset table and `_` comments are not
/// sources. Each compiles through [`stage_def`] (problems warned).
pub fn stage_defs(styles: &Json) -> HashMap<String, StageDef> {
    let Some(stages) = styles.get("stages").and_then(Json::as_object) else {
        return HashMap::new();
    };
    stages
        .keys()
        .filter(|k| is_source_key(k))
        .filter_map(|k| stage_def(styles, k).map(|d| (k.clone(), d)))
        .collect()
}

/// Whether a key of the `stages` block names a SOURCE: everything except the shared
/// `lighting` preset table and `_`-prefixed comments.
pub fn is_source_key(key: &str) -> bool {
    !key.starts_with('_') && key != "lighting"
}

/// The named lighting preset (`stages.lighting.<name>`), or `None` when unauthored.
pub fn lighting_preset(styles: &Json, name: &str) -> Option<LightRig> {
    let mut problems = Vec::new();
    let lighting = compile_preset(styles, name, &mut problems)?;
    for p in &problems {
        tracing::warn!("{p}");
    }
    Some(lighting)
}

/// Compile one source and return it WITH every problem found, as data — the gate's
/// entry point. `None` only when `stages.<source>` does not exist.
pub fn compile_stage(styles: &Json, source: &str) -> Option<(StageDef, Vec<String>)> {
    let stage = styles.get("stages")?.get(source)?;
    let mut problems = Vec::new();
    let mut def = StageDef::default();
    let Some(obj) = stage.as_object() else {
        problems.push(format!("stages.{source} is not an object"));
        return Some((def, problems));
    };
    // The images are read FIRST, whatever order they were authored in: a pass NAMES them,
    // so the recipe cannot be checked before they exist.
    if let Some(v) = obj.get("attachments") {
        def.attachments =
            compile_attachments(v, &format!("stages.{source}.attachments"), &mut problems);
    }
    for (key, v) in obj {
        let at = format!("stages.{source}.{key}");
        match key.as_str() {
            k if k.starts_with('_') => {}
            // Already read above — this arm is what keeps it out of the catch-all.
            "attachments" => {}
            "passes" => match v.as_array() {
                Some(passes) => {
                    let compiled: Vec<PassDef> = passes
                        .iter()
                        .enumerate()
                        .filter_map(|(i, p)| {
                            compile_pass(p, &format!("{at}[{i}]"), &def.attachments, &mut problems)
                        })
                        .collect();
                    def.passes = compiled;
                }
                None => problems.push(format!("{at} must be an array of passes")),
            },
            "rate" => def.rate = compile_rate(v, &at, &mut problems),
            "lighting" => match v.as_str() {
                Some(name) => match compile_preset(styles, name, &mut problems) {
                    Some(l) => def.lighting = l,
                    None => problems.push(format!(
                        "{at} names `{name}`, which stages.lighting does not author"
                    )),
                },
                None => problems.push(format!("{at} must name a stages.lighting preset")),
            },
            "clear" => {
                if let Some(c) = color4(v, &at, &mut problems) {
                    def.clear = Some(c.map(f64::from));
                }
            }
            "camera" => def.camera = Some(compile_camera(v, &at, &mut problems)),
            "layers" => match v.as_array() {
                Some(layers) => {
                    def.layers = layers
                        .iter()
                        .enumerate()
                        .filter_map(|(i, l)| compile_layer(l, &format!("{at}[{i}]"), &mut problems))
                        .collect();
                }
                None => problems.push(format!("{at} must be an array of layers")),
            },
            other => problems.push(format!("stages.{source}.{other} is not a stage key")),
        }
    }
    // An HDR surface and its tonemap are MUTUALLY REQUIRED. An `hdr` (rgba16f) attachment
    // is only ever the tonemap's source, and the tonemap is the only thing that resolves it
    // back to the surface colour — so one without the other is a half-wired surface that
    // renders nothing, or nothing that shows. Fail loud on both directions (rule 4BB12A75).
    let has_hdr = def
        .attachments
        .names()
        .filter_map(|n| def.attachments.get(n))
        .any(|a| a.format == AttachmentFormat::Rgba16f);
    let has_tonemap = def
        .recipe()
        .iter()
        .any(|p| matches!(p.kind, PassKind::TonemapGrade(_)));
    if has_hdr && !has_tonemap {
        problems.push(format!(
            "stages.{source} declares an rgba16f attachment but no `tonemap_grade` pass \
             resolves it back to the surface colour"
        ));
    }
    if has_tonemap && !has_hdr {
        problems.push(format!(
            "stages.{source}.passes runs `tonemap_grade`, but no rgba16f attachment feeds \
             it — there is nothing to tonemap"
        ));
    }
    // Bloom READS and WRITES the `hdr` attachment (it extracts the bright highlights and adds
    // the glow back before the tonemap), so a `bloom` pass on a surface with no rgba16f
    // attachment resolves to NOTHING — the same half-wired shape as a tonemap with no hdr. Fail
    // loud (rule 4BB12A75). A bloom without a tonemap is already covered: `has_hdr && !has_tonemap`
    // fires above, since a bloom stage must be HDR.
    let has_bloom = def
        .recipe()
        .iter()
        .any(|p| matches!(p.kind, PassKind::Bloom(_)));
    if has_bloom && !has_hdr {
        problems.push(format!(
            "stages.{source}.passes runs `bloom`, but no rgba16f attachment feeds it — there \
             is no HDR colour to extract the glow from"
        ));
    }
    // Every surface hands the graph a CONTENT closure, and the `scene` pass is where it
    // runs. An authored recipe that omits it would drop the surface's own drawing on the
    // floor in silence — the one shape of recipe that renders less than it was given.
    if !def.passes.is_empty() && !def.passes.iter().any(|p| p.kind == PassKind::Scene) {
        problems.push(format!(
            "stages.{source}.passes runs no `scene` pass — the surface's own content \
             would never be drawn"
        ));
    }
    // ── SHADOW-MAP COUPLING (fail loud, rule 4BB12A75) ──
    let has_lit_scene = def.recipe().iter().any(|p| p.kind == PassKind::Scene);
    let mut has_consumer_shadow = false;
    for p in def.recipe() {
        let PassKind::ShadowMap(s) = &p.kind else {
            continue;
        };
        // A shadow cast for a light the rig does not carry samples nothing.
        if s.light as usize >= def.lighting.count as usize {
            problems.push(format!(
                "stages.{source} casts a shadow for light {}, but its rig carries only {}",
                s.light, def.lighting.count
            ));
        }
        match &s.from {
            // CONSUMER: the named producer surface must be a known stage source.
            Some(from) => {
                has_consumer_shadow = true;
                let known =
                    styles.get("stages").and_then(|s| s.get(from)).is_some() && is_source_key(from);
                if !known {
                    problems.push(format!(
                        "stages.{source} binds a shadow from `{from}`, which is not a known \
                         stage source"
                    ));
                }
            }
            // PRODUCER: it must own the `depth` it renders the casters into.
            None => {
                if def.attachments.get(Attachments::DEPTH).is_none() {
                    problems.push(format!(
                        "stages.{source} produces a shadow_map but declares no `depth` \
                         attachment to render the casters into"
                    ));
                }
            }
        }
    }
    // A room that binds a shadow but has no lit pass to receive it is half-wired.
    if has_consumer_shadow && !has_lit_scene {
        problems.push(format!(
            "stages.{source} binds a shadow_map but runs no lit `scene` pass — a shadow is \
             bound and nothing lit reads it"
        ));
    }
    Some((def, problems))
}

/// `stages.lighting.<name>` → the frame's [`LightRig`]. Two authoring forms compile
/// into the SAME light list:
///
/// * the **legacy** keys — `sun_dir`/`sun`, `moon_dir`/`moon`, `point_pos`/`point` —
///   which always emit exactly three lights, in that order, black ones included, so a
///   preset written before the list existed reads identically;
/// * the **general** `lights: [ … ]` array, where each entry is
///   `{ kind, color, intensity, pos | dir, radius, cone: [inner, outer] (degrees),
///   driver: { kind, speed, depth, seed } }`.
///
/// The sky palette (`sky_zenith`/`sky_horizon`) and `ambient` belong to either form.
/// Fog is NOT authored here (the celestial cycle and the `ground_fog` pass own it), and
/// the colour GRADE is not a lighting key at all — it is pass-owned by `tonemap_grade`.
///
/// **The roster is SLOT-INDEXED, and slots 0/1 are the SKY SLOTS.** `lights[0]` is the
/// sun and `lights[1]` is the moon in ONE addressing scheme: the legacy keys compile
/// there, [`LightRig::sky_sun`]/`sky_moon` read exactly those two, and a celestial cycle
/// composing over the rig overwrites them BY INDEX every frame. A general-form array
/// that parks a non-`dir` light in either is therefore reported — the light would be
/// eaten by the cycle with nothing to show for it — and fixed lights belong at slot 2+.
fn compile_preset(styles: &Json, name: &str, problems: &mut Vec<String>) -> Option<LightRig> {
    let preset = styles.get("stages")?.get("lighting")?.get(name)?;
    let mut out = LightRig::default();
    let Some(obj) = preset.as_object() else {
        problems.push(format!("stages.lighting.{name} is not an object"));
        return Some(out);
    };
    // The legacy trio, seeded from the default rig's own slots so an unauthored half
    // (a `sun` with no `sun_dir`) keeps the value the default already chose.
    let mut legacy = [out.lights[0], out.lights[1], out.lights[2]];
    let mut authored_legacy = false;
    for (key, v) in obj {
        let at = format!("stages.lighting.{name}.{key}");
        let mut legacy_vec3 = |slot: usize, field: fn(&mut Light) -> &mut Vec3, unit: bool| {
            authored_legacy = true;
            let cur = *field(&mut legacy[slot]);
            let parsed = vec3(v, &at, cur, problems);
            *field(&mut legacy[slot]) = if unit {
                parsed.normalize_or_zero()
            } else {
                parsed
            };
        };
        match key.as_str() {
            k if k.starts_with('_') => {}
            "sun_dir" => legacy_vec3(0, |l| &mut l.direction, true),
            "sun" => legacy_vec3(0, |l| &mut l.color, false),
            "moon_dir" => legacy_vec3(1, |l| &mut l.direction, true),
            "moon" => legacy_vec3(1, |l| &mut l.color, false),
            // A point light is lit FROM a world position, so a rig whose light is a body
            // in the scene (a star at the origin) authors both halves or neither.
            "point_pos" => legacy_vec3(2, |l| &mut l.position, false),
            "point" => legacy_vec3(2, |l| &mut l.color, false),
            "ambient" => out.ambient = vec3(v, &at, out.ambient, problems),
            "sky_zenith" => out.sky_zenith = vec3(v, &at, out.sky_zenith, problems),
            "sky_horizon" => out.sky_horizon = vec3(v, &at, out.sky_horizon, problems),
            "lights" => {}
            other => problems.push(format!(
                "stages.lighting.{name}.{other} is not a lighting key"
            )),
        }
    }
    // The general form REPLACES the roster. Authoring both is a problem — but the array
    // still wins, because degrading to the legacy trio would black a stage that only
    // ever described its lights in the new vocabulary.
    match obj.get("lights") {
        Some(v) => {
            if authored_legacy {
                problems.push(format!(
                    "stages.lighting.{name} authors both the legacy `sun`/`moon`/`point` keys \
                     and a `lights` array — the array wins; delete one"
                ));
            }
            match v.as_array() {
                Some(entries) => {
                    // The array IS the roster from here — the legacy slots stand down.
                    out.count = 0;
                    out.lights = [Light::default(); MAX_LIGHTS];
                    if entries.is_empty() {
                        problems.push(format!(
                            "stages.lighting.{name}.lights is empty — an empty roster lights \
                             nothing; delete the key or author lights"
                        ));
                    }
                    if entries.len() > MAX_LIGHTS {
                        problems.push(format!(
                            "stages.lighting.{name}.lights has {} entries; a rig carries at \
                             most {MAX_LIGHTS} — the rest are dropped",
                            entries.len()
                        ));
                    }
                    for (i, e) in entries.iter().enumerate().take(MAX_LIGHTS) {
                        let at = format!("stages.lighting.{name}.lights[{i}]");
                        if let Some(light) = compile_light(e, &at, problems) {
                            out.push(light);
                        }
                    }
                    // Slots 0 and 1 are the SKY SLOTS — `sky_sun()`/`sky_moon()` read
                    // exactly those two, and a celestial cycle composing over this rig
                    // overwrites them BY INDEX every frame. A fixed light parked there
                    // is eaten with nothing to show for it, so it is loud here.
                    for i in 0..out.count.min(2) as usize {
                        if out.lights[i].kind != LightKind::Dir {
                            problems.push(format!(
                                "stages.lighting.{name}.lights slot {i} holds a `{}` light — \
                                 slots 0/1 are the sky slots the celestial cycle and the sky \
                                 shader read; put fixed lights at slot 2+",
                                kind_name(out.lights[i].kind)
                            ));
                        }
                    }
                }
                // Not an array at all: report it and keep the legacy trio standing, so a
                // malformed style file costs the authored look and never the picture.
                None => {
                    problems.push(format!(
                        "stages.lighting.{name}.lights must be an array of light objects"
                    ));
                    out.lights[..3].copy_from_slice(&legacy);
                    out.count = 3;
                }
            }
        }
        // No array: the legacy trio, always all three, in slot order.
        None => {
            out.lights[..3].copy_from_slice(&legacy);
            out.count = 3;
        }
    }
    Some(out)
}

/// The word an author types for a [`LightKind`] — the ONE place the enum spells itself
/// back, for the problems that name a kind.
fn kind_name(kind: LightKind) -> &'static str {
    match kind {
        LightKind::Dir => "dir",
        LightKind::Point => "point",
        LightKind::Spot => "spot",
    }
}

/// Every key a light entry may carry, across all kinds. [`compile_light`] narrows this
/// to the subset the resolved `kind` actually READS; the difference is what separates a
/// misspelling from a key that is spelled right and quietly does nothing.
const LIGHT_KEYS: [&str; 8] = [
    "kind",
    "color",
    "intensity",
    "pos",
    "dir",
    "radius",
    "cone",
    "driver",
];

/// One entry of a `lights` array. `None` drops that light (reported, never silent) —
/// the rest of the rig still stands.
fn compile_light(v: &Json, at: &str, problems: &mut Vec<String>) -> Option<Light> {
    let Some(obj) = v.as_object() else {
        problems.push(format!("{at} must be an object with a `kind`"));
        return None;
    };
    let kind = match obj.get("kind").and_then(Json::as_str) {
        Some("dir") => LightKind::Dir,
        Some("point") => LightKind::Point,
        Some("spot") => LightKind::Spot,
        Some(other) => {
            problems.push(format!(
                "{at}.kind names `{other}`, which is not a light kind (dir, point, spot)"
            ));
            return None;
        }
        None => {
            problems.push(format!("{at} has no `kind` (dir, point, spot)"));
            return None;
        }
    };
    let mut light = Light {
        kind,
        ..Light::default()
    };
    // The keys THIS kind reads — the checklist idiom `compile_layer` / `compile_pass`
    // already use. A key that is spelled right but inapplicable (`radius` or `cone` on
    // a `dir`, `pos` on a `dir`, `cone` on a `point`) is parsed, packed into the
    // uniform by `rig_to_uniform`, and then discarded by `light_sample` before it is
    // ever read: accepted-but-discarded is the hole, so it is a PROBLEM.
    let keys: &[&str] = match kind {
        LightKind::Dir => &["kind", "color", "intensity", "dir", "driver"],
        LightKind::Point => &["kind", "color", "intensity", "pos", "radius", "driver"],
        LightKind::Spot => &LIGHT_KEYS,
    };
    for key in obj.keys() {
        let key = key.as_str();
        if !key.starts_with('_') && !keys.contains(&key) && LIGHT_KEYS.contains(&key) {
            problems.push(format!(
                "{at}.{key} is not a key a `{}` light reads — it would be accepted here \
                 and then discarded by the shader",
                kind_name(kind)
            ));
        }
    }
    let (mut has_pos, mut has_dir, mut has_cone) = (false, false, false);
    for (key, value) in obj {
        let at = format!("{at}.{key}");
        match key.as_str() {
            k if k.starts_with('_') => {}
            "kind" => {}
            "color" => light.color = vec3(value, &at, light.color, problems),
            "intensity" => light.intensity = num(value, &at, light.intensity, problems),
            "pos" => {
                has_pos = true;
                light.position = vec3(value, &at, light.position, problems);
            }
            "dir" => {
                has_dir = true;
                light.direction = vec3(value, &at, light.direction, problems).normalize_or_zero();
            }
            "radius" => light.radius = num(value, &at, light.radius, problems),
            "cone" => {
                has_cone = true;
                let [inner, outer] = floats::<2>(value, &at, [0.0, 0.0], problems);
                light.cone_inner = inner.to_radians();
                light.cone_outer = outer.to_radians();
            }
            "driver" => light.driver = compile_driver(value, &at, problems),
            other => problems.push(format!("{at} is not a light key (`{other}`)")),
        }
    }
    // Each kind REQUIRES the fields it reads — a spot with no cone is a name that
    // resolves to nothing, so the light is dropped rather than shining somewhere wrong.
    let missing: Vec<&str> = match kind {
        LightKind::Dir => (!has_dir).then_some("dir").into_iter().collect(),
        LightKind::Point => (!has_pos).then_some("pos").into_iter().collect(),
        LightKind::Spot => [(!has_pos, "pos"), (!has_dir, "dir"), (!has_cone, "cone")]
            .into_iter()
            .filter_map(|(absent, k)| absent.then_some(k))
            .collect(),
    };
    if !missing.is_empty() {
        problems.push(format!(
            "{at} is a `{}` light with no {} — the light is dropped",
            kind_name(kind),
            missing.join(" / ")
        ));
        return None;
    }
    Some(light)
}

/// A light's `driver` block. `None` drops the driver — reported — and the light still
/// shines at its authored intensity.
fn compile_driver(v: &Json, at: &str, problems: &mut Vec<String>) -> Option<Driver> {
    let Some(obj) = v.as_object() else {
        problems.push(format!("{at} must be an object with a `kind`"));
        return None;
    };
    let kind = match obj.get("kind").and_then(Json::as_str) {
        Some("flicker") => DriverKind::Flicker,
        Some("pulse") => DriverKind::Pulse,
        Some(other) => {
            problems.push(format!(
                "{at}.kind names `{other}`, which is not a driver kind (flicker, pulse) — \
                 the light shines undriven"
            ));
            return None;
        }
        None => {
            problems.push(format!("{at} has no `kind` (flicker, pulse)"));
            return None;
        }
    };
    let mut driver = Driver {
        kind,
        speed: 1.0,
        depth: 0.0,
        seed: 0,
    };
    for (key, value) in obj {
        let at = format!("{at}.{key}");
        match key.as_str() {
            k if k.starts_with('_') => {}
            "kind" => {}
            "speed" => driver.speed = num(value, &at, driver.speed, problems),
            "depth" => driver.depth = num(value, &at, driver.depth, problems),
            "seed" => driver.seed = seed_num(value, &at, driver.seed, problems),
            other => problems.push(format!("{at} is not a driver key (`{other}`)")),
        }
    }
    Some(driver)
}

fn compile_camera(v: &Json, at: &str, problems: &mut Vec<String>) -> StageCamera {
    let mut cam = StageCamera::default();
    let Some(obj) = v.as_object() else {
        problems.push(format!("{at} must be an object"));
        return cam;
    };
    for (key, value) in obj {
        let at = format!("{at}.{key}");
        match key.as_str() {
            k if k.starts_with('_') => {}
            // The one framing kind today; a second kind is a new arm here, never a
            // silently-ignored string.
            "kind" => {
                if value.as_str() != Some("orbit") {
                    problems.push(format!("{at} must be \"orbit\" (the only framing kind)"));
                }
            }
            "yaw" => cam.yaw = num(value, &at, cam.yaw, problems),
            "pitch" => cam.pitch = num(value, &at, cam.pitch, problems),
            "dist" => cam.dist = num(value, &at, cam.dist, problems),
            "target_y" => cam.target_y = num(value, &at, cam.target_y, problems),
            other => problems.push(format!("{at} is not a camera key (`{other}`)")),
        }
    }
    cam
}

/// One `layers[]` entry. `None` drops the layer (no `draw`, or a kind the engine does not
/// know) — reported, never silent.
fn compile_layer(v: &Json, at: &str, problems: &mut Vec<String>) -> Option<StageLayer> {
    let Some(obj) = v.as_object() else {
        problems.push(format!("{at} must be an object with a `draw` kind"));
        return None;
    };
    let Some(kind) = obj.get("draw").and_then(Json::as_str) else {
        problems.push(format!("{at} has no `draw` kind"));
        return None;
    };
    // Read through a checklist of the keys this kind has, so a misspelled one is named.
    let mut keys: Vec<&str> = vec!["draw"];
    let mut number = |key: &'static str, default: f32, keys: &mut Vec<&str>| -> f32 {
        keys.push(key);
        match obj.get(key) {
            Some(n) => num(n, &format!("{at}.{key}"), default, problems),
            None => default,
        }
    };
    let layer = match kind {
        "skinned" => Some(StageLayer::Skinned),
        "ring" => {
            let radius = number("radius", 0.45, &mut keys);
            let y = number("y", 0.0, &mut keys);
            let segments = number("segments", 24.0, &mut keys).max(0.0) as usize;
            keys.extend(["color", "color_active"]);
            let color = obj
                .get("color")
                .and_then(|c| color4(c, &format!("{at}.color"), problems))
                .unwrap_or(RING_COLOR);
            // A source may omit the active colour; then the ring simply never lights.
            let color_active = obj
                .get("color_active")
                .and_then(|c| color4(c, &format!("{at}.color_active"), problems))
                .unwrap_or(color);
            Some(StageLayer::Ring {
                radius,
                y,
                segments,
                color,
                color_active,
            })
        }
        "grid" => {
            let spacing = number("spacing", 0.5, &mut keys);
            let extent = number("extent", 6.0, &mut keys);
            let y = number("y", 0.0, &mut keys);
            keys.push("color");
            let color = obj
                .get("color")
                .and_then(|c| color4(c, &format!("{at}.color"), problems))
                .unwrap_or(GRID_COLOR);
            Some(StageLayer::Grid {
                spacing,
                extent,
                y,
                color,
            })
        }
        "shells" => Some(StageLayer::Shells),
        "shell" => {
            let radius_scale = number("radius_scale", 1.0, &mut keys);
            let inset = number("inset", 0.0, &mut keys);
            keys.push("color");
            let c = obj
                .get("color")
                .and_then(|c| color4(c, &format!("{at}.color"), problems))
                .unwrap_or([1.0; 4]);
            Some(StageLayer::Shell {
                radius_scale,
                inset,
                color: [c[0], c[1], c[2]],
            })
        }
        "graticule" => Some(StageLayer::Graticule {
            radius_scale: number("radius_scale", 1.0, &mut keys),
        }),
        "material" => Some(StageLayer::Material),
        other => {
            problems.push(format!(
                "{at} draws `{other}`, which is not a layer kind the engine knows ({})",
                StageLayer::KINDS.join(", ")
            ));
            None
        }
    };
    for key in obj.keys() {
        if !key.starts_with('_') && !keys.contains(&key.as_str()) {
            problems.push(format!("{at} ({kind}) has no key `{key}`"));
        }
    }
    layer
}

/// `stages.<source>.attachments` — the images the surface owns, `name → {format, scale}`.
/// An authored block REPLACES the default colour+depth pair rather than adding to it, so
/// a stage that wants a half-resolution colour AND a depth declares both.
fn compile_attachments(v: &Json, at: &str, problems: &mut Vec<String>) -> Attachments {
    let Some(obj) = v.as_object() else {
        problems.push(format!(
            "{at} must be an object of name → {{ format, scale }}"
        ));
        return Attachments::default();
    };
    let mut out = Attachments::empty();
    for (name, spec) in obj {
        if name.starts_with('_') {
            continue;
        }
        let at = format!("{at}.{name}");
        let mut attachment = Attachment::default();
        let Some(spec) = spec.as_object() else {
            problems.push(format!("{at} must be an object of format and scale"));
            out.set(name, attachment);
            continue;
        };
        for (key, value) in spec {
            let at = format!("{at}.{key}");
            match key.as_str() {
                k if k.starts_with('_') => {}
                "format" => match value.as_str().and_then(AttachmentFormat::from_name) {
                    Some(f) => attachment.format = f,
                    None => problems.push(format!(
                        "{at} must be one of {}",
                        AttachmentFormat::NAMES.join(", ")
                    )),
                },
                "scale" => {
                    let scale = num(value, &at, attachment.scale, problems);
                    if scale > 0.0 {
                        attachment.scale = scale;
                    } else {
                        problems.push(format!("{at} must be greater than zero"));
                    }
                }
                other => problems.push(format!("{at} is not an attachment key (`{other}`)")),
            }
        }
        // `rgba16f` renders as of S3: a surface that declares one is the HDR intermediate a
        // `tonemap_grade` pass resolves. The hdr⟺tonemap coupling is enforced in
        // `compile_stage` (an rgba16f attachment with no tonemap, or vice versa, is a
        // problem there), so nothing is dropped silently.
        //
        // The `hdr` attachment carries two further rules, because it is the ONE image the
        // engine allocates from an authored format and resolves 1:1 (rule 4BB12A75):
        if name == Attachments::HDR {
            // The renderer allocates it through `AttachmentFormat::texture_format`, and the
            // tonemap pipeline reads a float texture. Any other format would compile here
            // and then be silently wrong on the GPU.
            if attachment.format != AttachmentFormat::Rgba16f {
                problems.push(format!(
                    "{at}.format is `{}` — the `hdr` attachment is the LINEAR HDR working \
                     colour the tonemap resolves, so it must be `{}`",
                    attachment.format.name(),
                    AttachmentFormat::Rgba16f.name()
                ));
            }
            // `Attachments::pixels` sizes every image of a surface off `color`'s scale, and
            // the tonemap resolve is a 1:1 `textureLoad` at framebuffer coords — so a scale
            // here is a number that changes nothing.
            if attachment.scale != 1.0 {
                problems.push(format!(
                    "{at}.scale is {} — the `hdr` attachment is sized off `color`'s scale \
                     and resolved 1:1 by the tonemap, so it must be 1.0",
                    attachment.scale
                ));
            }
        }
        out.set(name, attachment);
    }
    out
}

/// One `passes[]` entry. `None` drops the pass (no `pass` key, or a kind the engine does
/// not know) — reported, never silent, exactly as an unknown `draw` kind is.
fn compile_pass(
    v: &Json,
    at: &str,
    attachments: &Attachments,
    problems: &mut Vec<String>,
) -> Option<PassDef> {
    let Some(obj) = v.as_object() else {
        problems.push(format!("{at} must be an object with a `pass` kind"));
        return None;
    };
    let Some(kind_name) = obj.get("pass").and_then(Json::as_str) else {
        problems.push(format!("{at} has no `pass` kind"));
        return None;
    };
    // Read through a checklist of the keys this kind has, so a misspelled one is named.
    let mut keys: Vec<&str> = vec!["pass", "reads", "writes"];
    let mut number = |key: &'static str, default: f32, keys: &mut Vec<&str>| -> f32 {
        keys.push(key);
        match obj.get(key) {
            Some(n) => num(n, &format!("{at}.{key}"), default, problems),
            None => default,
        }
    };
    let kind = match kind_name {
        "scene" => Some(PassKind::Scene),
        "sky" => Some(PassKind::Sky),
        "volumetric_disk" => {
            let d = VolumetricDisk::default();
            let disk = VolumetricDisk {
                inner: number("inner", d.inner, &mut keys),
                outer: number("outer", d.outer, &mut keys),
                snow_line: number("snow_line", d.snow_line, &mut keys),
                scale_height: number("scale_height", d.scale_height, &mut keys),
                density: number("density", d.density, &mut keys),
                formation: number("formation", d.formation, &mut keys),
                time: number("time", d.time, &mut keys),
                tint: rgb(obj, at, "tint", d.tint, &mut keys, problems),
                glow: rgb(obj, at, "glow", d.glow, &mut keys, problems),
                // Never authored: the gaps are what the forming bodies carve, so they
                // arrive as a per-frame input or not at all.
                gaps: Vec::new(),
            };
            let binds = binds(
                obj,
                at,
                &[
                    ("formation_bind", VolumetricSlot::Formation),
                    ("time_bind", VolumetricSlot::Time),
                    ("density_bind", VolumetricSlot::Density),
                ],
                &mut keys,
                problems,
            );
            Some(PassKind::VolumetricDisk(Box::new(VolumetricPass {
                disk,
                binds,
            })))
        }
        "ground_fog" => {
            let d = GroundFog::default();
            let mut fog = GroundFog {
                bottom: number("bottom", d.bottom, &mut keys),
                top: number("top", d.top, &mut keys),
                density: number("density", d.density, &mut keys),
                noise_scale: number("noise_scale", d.noise_scale, &mut keys),
                coverage: number("coverage", d.coverage, &mut keys),
                height_power: number("height_power", d.height_power, &mut keys),
                edge_fade: number("edge_fade", d.edge_fade, &mut keys),
                fall_depth: number("fall_depth", d.fall_depth, &mut keys),
                flow: number("flow", d.flow, &mut keys),
                ..d
            };
            let floor = number("floor", 0.0, &mut keys);
            keys.extend(["wind", "bounds", "color"]);
            if let Some(w) = obj.get("wind") {
                let w = floats(w, &format!("{at}.wind"), [d.wind.x, d.wind.y], problems);
                fog.wind = Vec2::new(w[0], w[1]);
            }
            if let Some(b) = obj.get("bounds") {
                let b = floats(
                    b,
                    &format!("{at}.bounds"),
                    [
                        d.bounds_min.x,
                        d.bounds_min.y,
                        d.bounds_max.x,
                        d.bounds_max.y,
                    ],
                    problems,
                );
                fog.bounds_min = Vec2::new(b[0], b[1]);
                fog.bounds_max = Vec2::new(b[2], b[3]);
            }
            // Absent: the fog takes the renderer's LIVE fog colour, so fog under a
            // day/night cycle follows the cycle.
            let color = obj
                .get("color")
                .and_then(|c| color4(c, &format!("{at}.color"), problems))
                .map(|c| Vec3::new(c[0], c[1], c[2]));
            let binds = binds(
                obj,
                at,
                &[
                    ("floor_bind", FogSlot::Floor),
                    ("density_bind", FogSlot::Density),
                    ("time_bind", FogSlot::Time),
                    ("coverage_bind", FogSlot::Coverage),
                ],
                &mut keys,
                problems,
            );
            Some(PassKind::GroundFog(Box::new(GroundFogPass {
                fog,
                floor,
                color,
                binds,
            })))
        }
        "tonemap_grade" => {
            // Pass-owned grade: the tint (rgb), how far the resolve lerps toward it, and the
            // exposure before the curve. Neutral defaults (no tint, unit exposure) are a
            // pure ACES resolve. The grade rides HERE, not on the scene uniform. The numbers
            // are read before the `rgb` (which also borrows `problems`) so the `number`
            // closure's borrow ends first — the same ordering the volumetric arm uses.
            //
            // The TINT is pure art; the STRENGTH and the EXPOSURE are bindable, so a grade can
            // follow the simulation (a day/night cycle publishing a golden-hour warmth) instead
            // of sitting at one authored number all cycle. No binds = the authored numbers.
            let d = TonemapGradePass::default();
            let grade_strength = number("grade_strength", d.grade_strength, &mut keys);
            let exposure = number("exposure", d.exposure, &mut keys);
            let grade = rgb(obj, at, "grade", d.grade, &mut keys, problems);
            let binds = binds(
                obj,
                at,
                &[
                    ("grade_strength_bind", TonemapSlot::GradeStrength),
                    ("exposure_bind", TonemapSlot::Exposure),
                ],
                &mut keys,
                problems,
            );
            Some(PassKind::TonemapGrade(TonemapGradePass {
                grade,
                grade_strength,
                exposure,
                binds,
            }))
        }
        "composite" => {
            keys.push("from");
            match obj.get("from").and_then(Json::as_str) {
                Some(from) => Some(PassKind::Composite(CompositePass {
                    from: from.to_string(),
                })),
                None => {
                    problems.push(format!(
                        "{at} (composite) must name the surface it draws in `from`"
                    ));
                    None
                }
            }
        }
        // The sun/light shadow map — one kind, two roles. A PRODUCER (no `from`) authors
        // the light slot + the caster box `extent` + the sampling `bias`; a CONSUMER names
        // the producer surface in `from` (its `extent` is inert — the producer fitted the
        // box). Defaults are sensible art knobs a scene tunes in data (never in Rust).
        "shadow_map" => {
            let light = number("light", 0.0, &mut keys).max(0.0) as u32;
            let bias = number("bias", 0.0015, &mut keys);
            let extent = number("extent", 512.0, &mut keys);
            keys.push("from");
            let from = match obj.get("from") {
                None => None,
                Some(v) => match v.as_str() {
                    Some(s) if !s.is_empty() => Some(s.to_string()),
                    _ => {
                        problems.push(format!(
                            "{at} (shadow_map) `from` must name the producer surface it \
                             samples (a string), or be omitted for the producer role"
                        ));
                        None
                    }
                },
            };
            Some(PassKind::ShadowMap(ShadowMapPass {
                light,
                bias,
                extent,
                from,
            }))
        }
        // A REAL animated water MESH at a sea level — a PROJECTED-GRID ocean drawn as opaque
        // geometry (the VS casts a screen grid onto the sea plane, so it reaches the horizon with
        // no authored field box). Defaults are sourced from `Water::default()` (one
        // representation). Numbers are read before the `rgb` colours + the wave list so the
        // `number` closure's borrow of `problems` ends first (the ground_fog arm's ordering).
        // `wave_falloff` fades the far ocean flat; `env_strength` dials the Fresnel sky
        // reflection (the water is lit by the LIVE rig, so its look follows the cycle rather
        // than being authored per time of day); `wave_sources` is the ONE wave roster, holding
        // both radial (`center`) and directional (`dir`) sources.
        "water_surface" => {
            let d = Water::default();
            let sea_level = number("sea_level", d.sea_level, &mut keys);
            let shore_fade = number("shore_fade", d.shore_fade, &mut keys);
            let spec_shininess = number("spec_shininess", d.spec_shininess, &mut keys);
            let spec_strength = number("spec_strength", d.spec_strength, &mut keys);
            let normal_scale = number("normal_scale", d.normal_scale, &mut keys);
            let wave_falloff = number("wave_falloff", d.wave_falloff, &mut keys);
            let env_strength = number("env_strength", d.env_strength, &mut keys);
            let shallow = rgb(obj, at, "shallow", d.shallow, &mut keys, problems);
            let deep = rgb(obj, at, "deep", d.deep, &mut keys, problems);
            keys.push("wave_sources");
            let waves = wave_sources(obj, at, problems);
            let binds = binds(
                obj,
                at,
                &[
                    ("sea_level_bind", WaterSlot::SeaLevel),
                    ("time_bind", WaterSlot::Time),
                ],
                &mut keys,
                problems,
            );
            Some(PassKind::WaterSurface(Box::new(WaterPass {
                sea_level,
                shallow,
                deep,
                shore_fade,
                spec_shininess,
                spec_strength,
                normal_scale,
                wave_falloff,
                env_strength,
                waves,
                binds,
            })))
        }
        // The HDR bloom post-effect: bright HDR highlights glow. Pure art knobs, no binds and no
        // colours — the simplest arm, like `tonemap_grade`. Defaults from `BloomPass::default()`
        // (one representation). Out-of-range values that resolve to a physically wrong picture
        // are named (rule 4BB12A75): a NEGATIVE threshold blooms the whole frame, a negative
        // knee/radius inverts the ramp/spread, and a negative intensity SUBTRACTS light.
        "bloom" => {
            let d = BloomPass::default();
            let threshold = number("threshold", d.threshold, &mut keys);
            let knee = number("knee", d.knee, &mut keys);
            let intensity = number("intensity", d.intensity, &mut keys);
            let radius = number("radius", d.radius, &mut keys);
            if threshold < 0.0 {
                problems.push(format!(
                    "{at} (bloom) `threshold` must be >= 0 — a negative threshold blooms the \
                     entire frame; got {threshold}"
                ));
            }
            if knee < 0.0 {
                problems.push(format!(
                    "{at} (bloom) `knee` must be >= 0 — it is the soft-knee half-width below \
                     the threshold; got {knee}"
                ));
            }
            if intensity < 0.0 {
                problems.push(format!(
                    "{at} (bloom) `intensity` must be >= 0 — a negative intensity subtracts \
                     light from the scene; got {intensity}"
                ));
            }
            if radius < 0.0 {
                problems.push(format!(
                    "{at} (bloom) `radius` must be >= 0 — it scales the blur spread; got {radius}"
                ));
            }
            Some(PassKind::Bloom(BloomPass {
                threshold,
                knee,
                intensity,
                radius,
            }))
        }
        other => {
            problems.push(format!(
                "{at} runs `{other}`, which is not a pass kind the engine knows ({})",
                PassKind::KINDS.join(", ")
            ));
            None
        }
    };
    for key in obj.keys() {
        if !key.starts_with('_') && !keys.contains(&key.as_str()) {
            problems.push(format!("{at} ({kind_name}) has no key `{key}`"));
        }
    }
    let kind = kind?;
    // A pass's own reads/writes override the defaults its kind carries — the ONLY
    // ordering information a recipe holds, so every name must be an image that exists.
    let reads = declared(
        obj,
        at,
        "reads",
        attachments,
        PassDef::default_reads(&kind),
        problems,
    );
    let writes = declared(
        obj,
        at,
        "writes",
        attachments,
        PassDef::default_writes(&kind),
        problems,
    );
    // The tonemap's POSITION in the derived order is exactly its read of `hdr` — that read
    // is what puts it after everything the lit passes write. An authored `reads` that drops
    // it (`reads: ["depth"]`) would compile clean and then resolve at whatever spot
    // declaration order happened to give it, which is not an order anyone authored.
    if matches!(kind, PassKind::TonemapGrade(_)) && !reads.iter().any(|r| r == Attachments::HDR) {
        problems.push(format!(
            "{at} (tonemap_grade) must READ `{}` — that read is what makes it resolve LAST; \
             it reads {:?}",
            Attachments::HDR,
            reads
        ));
    }
    Some(PassDef {
        kind,
        reads,
        writes,
    })
}

/// `reads` / `writes`: an array of names this stage's `attachments` declare. Absent keeps
/// the kind's defaults; a name nothing declares is a problem and is dropped, so the
/// derived order never depends on an image that does not exist.
fn declared(
    obj: &serde_json::Map<String, Json>,
    at: &str,
    key: &str,
    attachments: &Attachments,
    default: Vec<String>,
    problems: &mut Vec<String>,
) -> Vec<String> {
    match obj.get(key) {
        None => default,
        Some(Json::Array(a)) => a
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let at = format!("{at}.{key}[{i}]");
                match n.as_str() {
                    Some(s) if attachments.get(s).is_some() => Some(s.to_string()),
                    Some(s) => {
                        problems.push(format!(
                            "{at} names `{s}`, which this stage's attachments do not declare ({})",
                            attachments.names().collect::<Vec<_>>().join(", ")
                        ));
                        None
                    }
                    None => {
                        problems.push(format!("{at} must be an attachment name"));
                        None
                    }
                }
            })
            .collect(),
        Some(_) => {
            problems.push(format!("{at}.{key} must be an array of attachment names"));
            default
        }
    }
}

/// The `*_bind` keys of one pass kind, in roster order — `(slot, published key)` pairs.
/// A bind REPLACES the authored field at apply time, so a number authored on the SAME
/// slot is dead data and is reported: one representation of a value per recipe.
fn binds<S: Copy>(
    obj: &serde_json::Map<String, Json>,
    at: &str,
    roster: &[(&'static str, S)],
    keys: &mut Vec<&'static str>,
    problems: &mut Vec<String>,
) -> Vec<(S, String)> {
    let mut out = Vec::new();
    for (key, slot) in roster {
        keys.push(key);
        if let Some(v) = obj.get(*key) {
            if let Some(k) = bind_key(v, &format!("{at}.{key}"), problems) {
                // Every roster key is its field's name plus `_bind` — the bind that
                // landed is the one that makes the field beside it unreachable.
                let field = key.trim_end_matches("_bind");
                if obj.contains_key(field) {
                    problems.push(format!(
                        "{at} `{field}` is authored AND bound by `{key}`; a bind REPLACES \
                         the field, so the number is dead data — author one"
                    ));
                }
                out.push((*slot, k));
            }
        }
    }
    out
}

/// The input key a `*_bind` names: a plain published name, never a `$token` (a colour
/// ref that reached here unresolved is the one way a `$` shows up in a string).
fn bind_key(v: &Json, at: &str, problems: &mut Vec<String>) -> Option<String> {
    match v.as_str() {
        Some(s) if !s.starts_with('$') && !s.is_empty() => Some(s.to_string()),
        Some(s) => {
            problems.push(format!(
                "{at} names `{s}`, which is not a key a scene can publish"
            ));
            None
        }
        None => {
            problems.push(format!("{at} must name the input key that drives it"));
            None
        }
    }
}

/// `wave_sources`: the wave roster of a `water_surface` pass — ONE array holding both kinds:
/// a RADIAL source `{ center: [x, z], amplitude, wavelength, speed, phase }` (rings from a
/// point — the near-shore chop) or a DIRECTIONAL one `{ dir: [dx, dz], … }` (a plane wave
/// marching along a world direction — the open-ocean swell that keeps the horizon moving).
/// `center` and `dir` are the SAME field of the [`WaveSource`] under two kinds, so an entry
/// names exactly one of them: both, or neither, is a problem rather than a silent winner.
/// Author-friendly `wavelength` / `speed` convert to the physics `k = 2π/λ` / `omega = speed·k`
/// the [`WaveSource`] stores, and an authored `dir` is NORMALIZED here (one representation — the
/// pass never carries both spellings, and `k·(p·dir)` is only a wavelength for a unit `dir`).
/// Absent = no waves (still water); a non-object entry, an unknown key, a zero `dir`, or more
/// than [`MAX_WAVE_SOURCES`] sources each fail loud.
fn wave_sources(
    obj: &serde_json::Map<String, Json>,
    at: &str,
    problems: &mut Vec<String>,
) -> Vec<WaveSource> {
    let Some(v) = obj.get("wave_sources") else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        problems.push(format!(
            "{at}.wave_sources must be an array of wave-source objects"
        ));
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, entry) in arr.iter().enumerate() {
        let at = format!("{at}.wave_sources[{i}]");
        let Some(o) = entry.as_object() else {
            problems.push(format!(
                "{at} must be an object with center-or-dir/amplitude/wavelength/speed/phase"
            ));
            continue;
        };
        let kind = match (o.get("center"), o.get("dir")) {
            (Some(c), None) => {
                let c = floats(c, &format!("{at}.center"), [0.0, 0.0], problems);
                WaveKind::Radial {
                    center: Vec2::new(c[0], c[1]),
                }
            }
            (None, Some(d)) => {
                let d = floats(d, &format!("{at}.dir"), [1.0, 0.0], problems);
                let dir = Vec2::new(d[0], d[1]);
                // Normalized ONCE, here at the seam, exactly as `wavelength` becomes `k`: the
                // shader's plane-wave phase `k·(p·dir)` is only the authored wavelength when
                // `dir` is unit, and a zero direction has no wave to travel along at all.
                if dir.length() < 1e-4 {
                    problems.push(format!(
                        "{at}.dir must be a non-zero [dx, dz] direction — a zero-length \
                         direction has no plane wave to travel along"
                    ));
                    WaveKind::default()
                } else {
                    WaveKind::Directional {
                        dir: dir.normalize(),
                    }
                }
            }
            (Some(_), Some(_)) => {
                problems.push(format!(
                    "{at} names BOTH `center` and `dir` — a wave source is EITHER radial \
                     (rings from a `center`) or directional (a plane wave along `dir`), \
                     never both"
                ));
                WaveKind::default()
            }
            (None, None) => {
                problems.push(format!(
                    "{at} must name either its `center` [x, z] (a radial source) or its \
                     `dir` [dx, dz] (a directional open-ocean plane wave)"
                ));
                WaveKind::default()
            }
        };
        let amplitude = o
            .get("amplitude")
            .map_or(0.0, |n| num(n, &format!("{at}.amplitude"), 0.0, problems));
        let wavelength = o
            .get("wavelength")
            .map_or(40.0, |n| {
                num(n, &format!("{at}.wavelength"), 40.0, problems)
            })
            .max(1e-3);
        let speed = o
            .get("speed")
            .map_or(0.0, |n| num(n, &format!("{at}.speed"), 0.0, problems));
        let phase = o
            .get("phase")
            .map_or(0.0, |n| num(n, &format!("{at}.phase"), 0.0, problems));
        // A typo'd wave-source key is loud, not a silently inert wave (rule 4BB12A75).
        for key in o.keys() {
            if !key.starts_with('_')
                && !["center", "dir", "amplitude", "wavelength", "speed", "phase"]
                    .contains(&key.as_str())
            {
                problems.push(format!("{at} has no key `{key}`"));
            }
        }
        if out.len() >= MAX_WAVE_SOURCES {
            problems.push(format!(
                "{at}: a water_surface sums at most {MAX_WAVE_SOURCES} wave sources"
            ));
            continue;
        }
        let k = std::f32::consts::TAU / wavelength;
        out.push(WaveSource {
            kind,
            amplitude,
            k,
            omega: speed * k,
            phase,
        });
    }
    out
}

/// An already token-resolved rgba, truncated to the linear RGB a light or a tint is.
fn rgb(
    obj: &serde_json::Map<String, Json>,
    at: &str,
    key: &'static str,
    default: Vec3,
    keys: &mut Vec<&'static str>,
    problems: &mut Vec<String>,
) -> Vec3 {
    keys.push(key);
    obj.get(key)
        .and_then(|c| color4(c, &format!("{at}.{key}"), problems))
        .map(|c| Vec3::new(c[0], c[1], c[2]))
        .unwrap_or(default)
}

/// `stages.<source>.rate` — how often a surface of this stage re-renders. One word, or
/// `{"hz": N}`. Public because the walker parses a NODE's `rate` prop through the same
/// reader: one vocabulary, one set of problems, wherever liveness is authored.
pub fn compile_rate(v: &Json, at: &str, problems: &mut Vec<String>) -> Rate {
    // Every rate is live: the per-surface clock (S5d) drives `Rate::renders` once a frame at
    // each surface, so `dirty` and `hz` re-render like `live`/`poster` and are authorable.
    // Only a non-positive `hz` is still a problem (it would divide by zero, not "never").
    match v {
        Json::String(s) => match s.as_str() {
            "live" => Rate::Live,
            "poster" => Rate::Poster,
            "dirty" => Rate::Dirty,
            other => {
                problems.push(format!(
                    "{at} names `{other}`, which is not a rate ({}, or {{ \"hz\": N }})",
                    Rate::NAMES.join(", ")
                ));
                Rate::Live
            }
        },
        Json::Object(obj) => {
            let mut rate = Rate::Live;
            for (key, value) in obj {
                let at = format!("{at}.{key}");
                match key.as_str() {
                    k if k.starts_with('_') => {}
                    "hz" => {
                        let hz = num(value, &at, 0.0, problems);
                        if hz > 0.0 {
                            rate = Rate::Hz(hz);
                        } else {
                            problems.push(format!("{at} must be greater than zero"));
                        }
                    }
                    other => problems.push(format!("{at} is not a rate key (`{other}`)")),
                }
            }
            rate
        }
        _ => {
            problems.push(format!(
                "{at} must be one of {} — or {{ \"hz\": N }}",
                Rate::NAMES.join(", ")
            ));
            Rate::Live
        }
    }
}

/// The pack editor's gold ground ring — what a `ring` layer draws in when it authors
/// no colour.
const RING_COLOR: [f32; 4] = [0.72, 0.59, 0.35, 1.0];
/// The faint floor grid — what a `grid` layer draws in when it authors no colour.
const GRID_COLOR: [f32; 4] = [0.55, 0.63, 0.75, 0.09];

/// A finite number, else `default` and a problem.
fn num(v: &Json, at: &str, default: f32, problems: &mut Vec<String>) -> f32 {
    match v.as_f64().map(|n| n as f32).filter(|n| n.is_finite()) {
        Some(n) => n,
        None => {
            problems.push(format!("{at} must be a finite number"));
            default
        }
    }
}

/// A driver `seed` — the ONE integer in the lighting vocabulary, read as an integer.
/// NOT through [`num`]: an f32 carries 24 bits of mantissa, so `16_777_217` would land
/// on `16_777_216` and two lamps authored with adjacent large seeds would run in
/// lockstep — the exact thing a seed exists to prevent. A negative, a fraction, or a
/// value past [`u32::MAX`] is a problem rather than something to clamp and truncate; a
/// JSON `1.0` is the same whole number differently spelled and is accepted.
fn seed_num(v: &Json, at: &str, default: u32, problems: &mut Vec<String>) -> u32 {
    let whole = v.as_u64().or_else(|| {
        v.as_f64()
            .filter(|n| n.fract() == 0.0 && *n >= 0.0)
            .map(|n| n as u64)
    });
    match whole.filter(|n| *n <= u32::MAX as u64) {
        Some(n) => n as u32,
        None => {
            problems.push(format!(
                "{at} must be a whole number in 0..={} — a driver seed is an integer",
                u32::MAX
            ));
            default
        }
    }
}

/// Exactly `N` finite numbers, else `default` and a problem — the fog's wind (2) and the
/// XZ rectangle it is localized to (4).
fn floats<const N: usize>(
    v: &Json,
    at: &str,
    default: [f32; N],
    problems: &mut Vec<String>,
) -> [f32; N] {
    let parsed = v
        .as_array()
        .filter(|a| a.len() == N)
        .map(|a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(f64::NAN) as f32));
    match parsed.filter(|c: &[f32; N]| c.iter().all(|x| x.is_finite())) {
        Some(out) => out,
        None => {
            problems.push(format!("{at} must be {N} finite numbers"));
            default
        }
    }
}

/// A finite 3-vector, else `default` and a problem.
fn vec3(v: &Json, at: &str, default: Vec3, problems: &mut Vec<String>) -> Vec3 {
    let parsed = v.as_array().filter(|a| a.len() >= 3).map(|a| {
        Vec3::new(
            a[0].as_f64().unwrap_or(f64::NAN) as f32,
            a[1].as_f64().unwrap_or(f64::NAN) as f32,
            a[2].as_f64().unwrap_or(f64::NAN) as f32,
        )
    });
    match parsed.filter(|v| v.is_finite()) {
        Some(out) => out,
        None => {
            problems.push(format!("{at} must be three finite numbers"));
            default
        }
    }
}

/// An already token-resolved rgba. A string here is a `$token` the palette never
/// defined — the one way a colour reaches this compiler unresolved.
fn color4(v: &Json, at: &str, problems: &mut Vec<String>) -> Option<[f32; 4]> {
    match v {
        Json::Array(a) if a.len() >= 4 => {
            let c: [f32; 4] = std::array::from_fn(|i| a[i].as_f64().unwrap_or(f64::NAN) as f32);
            if c.iter().all(|x| x.is_finite()) {
                Some(c)
            } else {
                problems.push(format!("{at} must be four finite numbers"));
                None
            }
        }
        Json::String(s) if s.starts_with('$') => {
            problems.push(format!(
                "{at} names `{s}`, a token the palette does not define"
            ));
            None
        }
        _ => {
            problems.push(format!("{at} must be a $token colour or four numbers"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use flicker_script::UiNode;

    fn content_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/sensorium")
    }

    fn theme_path() -> std::path::PathBuf {
        content_root().join("resources/ui_theme.json")
    }

    /// The REAL shipped styles root — satellites merged, tokens resolved — exactly as
    /// the runtime builds them for a scene that authors no blocks of its own.
    fn shipped_styles() -> Json {
        crate::load_styles(theme_path())
    }

    /// The `.scene.json` files directly inside `dir`, sorted.
    fn scene_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{} reads: {e}", dir.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".scene.json"))
            })
            .collect();
        names.sort();
        names
    }

    /// Every shipped MANIFEST scene (the top-level folder — what `SceneManifest`
    /// loads; `shared/` holds shell furniture fragments with no behaviour of their own),
    /// parsed, with the styles root the runtime would build for it (its own `stages`
    /// merged in).
    fn shipped_scenes() -> Vec<(String, crate::SceneDef, Json)> {
        let mut out = Vec::new();
        for path in scene_files(&content_root().join("scenes")) {
            let id = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap()
                .trim_end_matches(".scene.json")
                .to_string();
            let text = std::fs::read_to_string(&path).expect("scene file reads");
            let def = crate::SceneDef::parse(&id, &text)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let styles = crate::load_styles_for(theme_path(), def.styles.as_ref());
            out.push((id, def, styles));
        }
        assert!(out.len() > 10, "the shipped scene folder was read");
        out
    }

    /// The `surface` nodes of the `shared/` fragments, as raw JSON (they are not
    /// manifest scenes, so they do not parse as a `SceneDef`): `(file, node id, source)`
    /// for every one that names a source.
    fn shared_fragment_sources() -> Vec<(String, String, String)> {
        fn walk_json(v: &Json, file: &str, out: &mut Vec<(String, String, String)>) {
            if let Some(obj) = v.as_object() {
                if obj.get("component").and_then(Json::as_str) == Some("surface") {
                    if let Some(source) = obj.get("source").and_then(Json::as_str) {
                        let id = obj.get("id").and_then(Json::as_str).unwrap_or("");
                        out.push((file.to_string(), id.to_string(), source.to_string()));
                    }
                }
                for child in obj.values() {
                    walk_json(child, file, out);
                }
            } else if let Some(arr) = v.as_array() {
                for child in arr {
                    walk_json(child, file, out);
                }
            }
        }
        let mut out = Vec::new();
        for path in scene_files(&content_root().join("scenes/shared")) {
            let text = std::fs::read_to_string(&path).expect("fragment reads");
            let json: Json = serde_json::from_str(&text).expect("fragment parses");
            let file = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap()
                .to_string();
            walk_json(&json, &file, &mut out);
        }
        out
    }

    fn walk(n: &UiNode, f: &mut impl FnMut(&UiNode)) {
        f(n);
        for c in &n.children {
            walk(c, f);
        }
    }

    /// The `source` a surface node names, if any.
    fn source_of(n: &UiNode) -> Option<&str> {
        match n.props.get("source") {
            Some(flicker_script::Value::Text(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    fn assert_compiles_clean(styles: &Json, source: &str, where_: &str) {
        let (def, problems) =
            compile_stage(styles, source).unwrap_or_else(|| panic!("{where_}: `{source}` exists"));
        assert!(
            problems.is_empty(),
            "{where_} stages.{source} has authoring problems:\n  {}",
            problems.join("\n  ")
        );
        let lum = |v: Vec3| v.x + v.y + v.z;
        assert!(
            lum(def.lighting.sky_sun().radiance()) > 0.0 || lum(def.lighting.ambient) > 0.0,
            "{where_} stages.{source} would render black"
        );
        // A stage draws either CONTENT layers or engine PASSES — a source with neither
        // says nothing about what its surface renders.
        assert!(
            !def.layers.is_empty() || !def.passes.is_empty(),
            "{where_} stages.{source} draws nothing"
        );
    }

    /// **GATE — every shipped stage compiles with NO problems**, wherever it lives:
    /// the shared library (`ui_stages.json`) and every scene file's own `stages`
    /// section. An unknown key, draw kind or preset, or a colour whose `$token` never
    /// resolved, fails the build here with its path — instead of a warning at runtime
    /// that nobody is watching for.
    #[test]
    fn every_shipped_stage_compiles_clean() {
        let styles = shipped_styles();
        let stages = styles["stages"]
            .as_object()
            .expect("the shipped stages block");
        for source in stages.keys().filter(|k| is_source_key(k)) {
            assert_compiles_clean(&styles, source, "ui_stages.json");
        }
        // Every preset compiles clean too, including the one the `night` sky rides.
        let presets = stages["lighting"].as_object().expect("the preset table");
        assert!(!presets.is_empty(), "the library authors lighting presets");
        for name in presets.keys().filter(|n| !n.starts_with('_')) {
            let mut problems = Vec::new();
            compile_preset(&styles, name, &mut problems).expect("preset exists");
            assert!(problems.is_empty(), "stages.lighting.{name}: {problems:?}");
        }
        // And each scene's own stages, compiled against the root the runtime builds
        // for THAT scene.
        let mut scene_stages = 0;
        for (id, def, styles) in shipped_scenes() {
            for source in def.stages().into_iter().flat_map(|s| s.keys()) {
                assert_compiles_clean(&styles, source, &format!("{id}.scene.json"));
                scene_stages += 1;
            }
        }
        assert!(
            scene_stages > 0,
            "the shipped scenes author stages of their own"
        );
    }

    /// **GATE — the sun-shadow `shadow_map` parser: the two roles compile and every misuse
    /// is a NAMED problem (rule 4BB12A75).** A PRODUCER (no `from`) writes the `depth` it
    /// renders casters into and carries its authored `extent`/`bias`; a CONSUMER (a `from`
    /// naming it) contributes an input binding (no reads/writes). A light past the rig, a
    /// `from` resolving to nothing, and a bound shadow with no lit `scene` pass each fail
    /// loud — the difference between an authorable knob and a silent no-op.
    #[test]
    fn the_shadow_map_pass_compiles_its_roles_and_names_its_misuse() {
        let styles = serde_json::json!({
            "stages": {
                "lighting": { "day": { "sun_dir": [0.4, 0.8, 0.5], "sun": [1.0, 1.0, 1.0] } },
                "sun_shadow": {
                    "lighting": "day",
                    "rate": { "hz": 20 },
                    "passes": [
                        { "pass": "shadow_map", "light": 0, "bias": 0.002, "extent": 500 },
                        { "pass": "scene" }
                    ]
                },
                "room": {
                    "lighting": "day",
                    "passes": [
                        { "pass": "shadow_map", "light": 0, "from": "sun_shadow", "bias": 0.002 },
                        { "pass": "scene" }
                    ]
                }
            }
        });

        // PRODUCER: clean, carries the knobs, writes the shadow depth.
        let (producer, p) = compile_stage(&styles, "sun_shadow").expect("compiles");
        assert!(p.is_empty(), "producer clean: {p:?}");
        let prod_pass = &producer.recipe()[0];
        let PassKind::ShadowMap(s) = &prod_pass.kind else {
            panic!("the first pass is the producer shadow_map");
        };
        assert_eq!((s.light, s.from.clone()), (0, None));
        assert!((s.extent - 500.0).abs() < 1e-6 && (s.bias - 0.002).abs() < 1e-6);
        assert_eq!(
            prod_pass.writes,
            ["depth"],
            "the producer produces the shadow depth"
        );

        // CONSUMER: clean, contributes an input binding (empty reads/writes).
        let (room, r) = compile_stage(&styles, "room").expect("compiles");
        assert!(r.is_empty(), "consumer clean: {r:?}");
        let consumer = room
            .recipe()
            .iter()
            .find(|p| matches!(&p.kind, PassKind::ShadowMap(s) if s.from.is_some()))
            .expect("the consumer shadow_map");
        assert!(
            consumer.reads.is_empty() && consumer.writes.is_empty(),
            "a consumer binds a foreign surface, it does not read/write this one's attachments"
        );

        // A LIGHT PAST THE RIG → problem.
        let bad_light = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "shadow_map", "light": 9, "extent": 100 }, { "pass": "scene" }
            ] } } });
        let (_, p) = compile_stage(&bad_light, "s").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("casts a shadow for light 9")),
            "a shadow for a light the rig lacks must be loud: {p:?}"
        );

        // A CONSUMER `from` NAMING NO SOURCE → problem.
        let bad_from = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "shadow_map", "light": 0, "from": "ghost" }, { "pass": "scene" }
            ] } } });
        let (_, p) = compile_stage(&bad_from, "s").expect("compiles");
        assert!(
            p.iter()
                .any(|m| m.contains("`ghost`") && m.contains("not a known stage source")),
            "a shadow bound from an unknown surface must be loud: {p:?}"
        );

        // A BOUND SHADOW WITH NO LIT `scene` PASS → problem.
        let no_scene = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "producer": { "lighting": "day", "passes": [
                { "pass": "shadow_map", "light": 0 }, { "pass": "scene" } ] },
            "s": { "lighting": "day", "passes": [
                { "pass": "shadow_map", "light": 0, "from": "producer" }, { "pass": "sky" }
            ] } } });
        let (_, p) = compile_stage(&no_scene, "s").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("nothing lit reads it")),
            "a shadow bound with no lit pass to receive it must be loud: {p:?}"
        );
    }

    /// **GATE — the `water_surface` parser: a synthetic water stage compiles clean and
    /// orders correctly, and every misuse is a NAMED problem (rule 4BB12A75).** The water pass
    /// derives AFTER `scene` (it reads the depth the scene writes) and BEFORE `tonemap_grade`
    /// (it writes the `hdr` the tonemap reads), carries its wave roster of BOTH kinds — radial
    /// (`center`) and directional (`dir`, normalized here) — with author-friendly
    /// `wavelength`/`speed` converted to the physics `k`/`omega`, and a bad wave-source key, a
    /// misspelled key, and a `sea_level` authored AND bound each fail loud.
    #[test]
    fn the_water_surface_pass_compiles_and_names_its_misuse() {
        let styles = serde_json::json!({
            "stages": {
                "lighting": { "day": { "sun_dir": [0.4, 0.8, 0.5], "sun": [1.0, 1.0, 1.0] } },
                "lake": {
                    "lighting": "day",
                    "attachments": {
                        "color": {}, "depth": { "format": "depth32" },
                        "hdr": { "format": "rgba16f" }
                    },
                    "passes": [
                        { "pass": "scene", "writes": ["hdr", "depth"] },
                        {
                            "pass": "water_surface", "sea_level": 2.0, "shore_fade": 5.0,
                            "spec_shininess": 300.0, "spec_strength": 1.5, "wave_falloff": 0.01,
                            "shallow": [0.1, 0.3, 0.4, 1.0], "deep": [0.0, 0.05, 0.1, 1.0],
                            "wave_sources": [
                                { "center": [50, 50], "amplitude": 1.0, "wavelength": 40, "speed": 2, "phase": 0.5 },
                                { "center": [10, 90], "amplitude": 0.5, "wavelength": 80, "speed": 1 },
                                { "dir": [3, 4], "amplitude": 0.4, "wavelength": 160, "speed": 12 }
                            ],
                            "time_bind": "clock"
                        },
                        { "pass": "tonemap_grade" }
                    ]
                }
            }
        });

        // CLEAN, and the derived order is scene → water → tonemap.
        let (def, p) = compile_stage(&styles, "lake").expect("compiles");
        assert!(p.is_empty(), "water stage clean: {p:?}");
        let (order, cyclic) = def.pass_order();
        assert!(!cyclic);
        let kinds: Vec<&str> = order.iter().map(|&i| def.recipe()[i].kind.kind()).collect();
        assert_eq!(
            kinds,
            ["scene", "water_surface", "tonemap_grade"],
            "water follows the scene and precedes the tonemap: {kinds:?}"
        );
        // The pass carried its authored knobs, wave roster (λ/speed → k/ω), and the time bind.
        let PassKind::WaterSurface(w) = &def.recipe()[order[1]].kind else {
            panic!("the second pass is the water_surface");
        };
        assert!((w.sea_level - 2.0).abs() < 1e-6 && (w.shore_fade - 5.0).abs() < 1e-6);
        assert!((w.spec_shininess - 300.0).abs() < 1e-6 && (w.spec_strength - 1.5).abs() < 1e-6);
        assert!(
            (w.wave_falloff - 0.01).abs() < 1e-6,
            "the far-field wave falloff is authored"
        );
        assert_eq!(w.waves.len(), 3, "every wave source parsed");
        let s0 = w.waves[0];
        assert_eq!(
            s0.kind,
            WaveKind::Radial {
                center: Vec2::new(50.0, 50.0)
            },
            "an authored `center` is a RADIAL source"
        );
        assert!(
            (s0.k - std::f32::consts::TAU / 40.0).abs() < 1e-5,
            "k = 2π/wavelength"
        );
        assert!(
            (s0.omega - 2.0 * std::f32::consts::TAU / 40.0).abs() < 1e-5,
            "omega = speed·k"
        );
        // The DIRECTIONAL entry: an authored `dir` becomes the other kind, NORMALIZED at parse
        // (3,4 → 0.6,0.8) — `k·(p·dir)` is only the authored wavelength for a unit direction.
        let WaveKind::Directional { dir } = w.waves[2].kind else {
            panic!(
                "an authored `dir` is a DIRECTIONAL source: {:?}",
                w.waves[2]
            );
        };
        assert!(
            (dir - Vec2::new(0.6, 0.8)).length() < 1e-5,
            "the authored `dir` is normalized at parse: {dir:?}"
        );
        assert!(
            (w.waves[2].k - std::f32::consts::TAU / 160.0).abs() < 1e-5
                && (w.waves[2].omega - 12.0 * std::f32::consts::TAU / 160.0).abs() < 1e-5,
            "a directional source converts λ/speed the same way a radial one does"
        );
        assert_eq!(w.binds, vec![(WaterSlot::Time, "clock".to_string())]);
        assert_eq!(
            def.recipe()[order[1]].reads,
            ["depth"],
            "water reads the scene depth"
        );
        assert_eq!(
            def.recipe()[order[1]].writes,
            ["hdr"],
            "water writes the hdr colour the tonemap resolves"
        );

        // A TYPO'D WAVE-SOURCE KEY → problem (named, not a silently inert wave).
        let bad_wave = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "water_surface", "wave_sources": [
                    { "center": [0, 0], "amplitud": 1.0 }
                ] }
            ] } } });
        let (_, p) = compile_stage(&bad_wave, "s").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("`amplitud`")),
            "a misspelled wave-source key must be named: {p:?}"
        );

        // A source naming BOTH `center` and `dir` → problem. The two spellings are ONE field
        // under two kinds; "accept both and pick a winner" is exactly the silent fork the
        // one-representation law forbids.
        let both = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "water_surface", "wave_sources": [
                    { "center": [0, 0], "dir": [1, 0], "amplitude": 1.0 }
                ] }
            ] } } });
        let (_, p) = compile_stage(&both, "s").expect("compiles");
        assert!(
            p.iter()
                .any(|m| m.contains("BOTH `center` and `dir`") && m.contains("never both")),
            "a wave source naming both a centre and a direction must be loud: {p:?}"
        );

        // A source naming NEITHER → problem (it has no geometry at all, and would otherwise be
        // a silent ring centred on the world origin).
        let neither = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "water_surface", "wave_sources": [
                    { "amplitude": 1.0, "wavelength": 30 }
                ] }
            ] } } });
        let (_, p) = compile_stage(&neither, "s").expect("compiles");
        assert!(
            p.iter()
                .any(|m| m.contains("`center`") && m.contains("`dir`")),
            "a wave source with neither a centre nor a direction must be loud: {p:?}"
        );

        // A ZERO `dir` → problem: `k·(p·dir)` collapses and there is no wave to travel along.
        let zero_dir = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "water_surface", "wave_sources": [
                    { "dir": [0, 0], "amplitude": 1.0, "wavelength": 150 }
                ] }
            ] } } });
        let (_, p) = compile_stage(&zero_dir, "s").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("non-zero [dx, dz]")),
            "a zero-length wave direction must be loud: {p:?}"
        );

        // MORE THAN `MAX_WAVE_SOURCES` sources → problem on the overflow entry (the roster is a
        // fixed uniform array; a silently dropped wave is a wave an author cannot find).
        let over: Vec<Json> = (0..MAX_WAVE_SOURCES + 1)
            .map(|i| serde_json::json!({ "center": [i, 0], "amplitude": 0.5 }))
            .collect();
        let crowded = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "water_surface", "wave_sources": over }
            ] } } });
        let (def_over, p) = compile_stage(&crowded, "s").expect("compiles");
        assert!(
            p.iter()
                .any(|m| m.contains(&format!("sums at most {MAX_WAVE_SOURCES} wave sources"))),
            "a {}th wave source must be loud: {p:?}",
            MAX_WAVE_SOURCES + 1
        );
        let Some(PassKind::WaterSurface(w_over)) = def_over
            .recipe()
            .iter()
            .map(|d| &d.kind)
            .find(|k| matches!(k, PassKind::WaterSurface(_)))
        else {
            panic!("the crowded stage still carries its water pass");
        };
        assert_eq!(
            w_over.waves.len(),
            MAX_WAVE_SOURCES,
            "the roster is capped at MAX_WAVE_SOURCES"
        );

        // A MISSPELLED KEY → problem.
        let typo = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "water_surface", "sea_levle": 2.0 }
            ] } } });
        let (_, p) = compile_stage(&typo, "s").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("`sea_levle`")),
            "a misspelled water key must be named: {p:?}"
        );

        // A SEA LEVEL AUTHORED **AND** BOUND → dead-data problem (the binds() gate).
        let dead = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "water_surface", "sea_level": 2.0, "sea_level_bind": "flood" }
            ] } } });
        let (_, p) = compile_stage(&dead, "s").expect("compiles");
        assert!(
            p.iter()
                .any(|m| m.contains("`sea_level`") && m.contains("authored AND bound")),
            "a sea_level authored and bound must be loud: {p:?}"
        );

        // The RETIRED `bounds` field box (the ocean is a projected grid now, everywhere the sea
        // plane is visible) → an authored `bounds` is now an UNKNOWN key, named loud, not
        // silently ignored (rule 4BB12A75).
        let boxed = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "water_surface", "bounds": [0, 0, 100, 100] }
            ] } } });
        let (_, p) = compile_stage(&boxed, "s").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("`bounds`")),
            "the retired field box `bounds` must be named as unknown: {p:?}"
        );
    }

    /// **GATE — the `tonemap_grade` parser BINDS its strength and exposure, and every misuse
    /// is a NAMED problem (rule 4BB12A75).** The grade used to be static — the ONE pass kind
    /// with no `*_bind` slots — so a room under a day/night cycle graded the same at noon and
    /// at sunset. This proves the `*_bind` checklist landed on it exactly like the fog's and
    /// the water's: the keys parse into slots, the AUTHORED TINT survives beside them (the tint
    /// is art, the strength is per-frame state), a number authored on a bound slot is the
    /// dead-data problem, a misspelled key is named, and a stage authoring NO binds keeps its
    /// authored numbers — the guarantee for every tonemap written before binds existed.
    #[test]
    fn the_tonemap_grade_pass_binds_its_strength_and_exposure() {
        let hdr_stage = |tonemap: serde_json::Value| {
            serde_json::json!({ "stages": {
                "lighting": { "day": { "sun_dir": [0.4, 0.8, 0.5], "sun": [1.0, 1.0, 1.0] } },
                "room": {
                    "lighting": "day",
                    "attachments": {
                        "color": {}, "depth": { "format": "depth32" },
                        "hdr": { "format": "rgba16f" }
                    },
                    "passes": [
                        { "pass": "scene", "writes": ["hdr", "depth"] },
                        tonemap
                    ]
                }
            } })
        };

        // CLEAN: an authored golden TINT with its strength + exposure BOUND — the shipped
        // Prism Test Room shape.
        let styles = hdr_stage(serde_json::json!({
            "pass": "tonemap_grade",
            "grade": [1.18, 0.92, 0.68, 1.0],
            "grade_strength_bind": "grade_warmth",
            "exposure_bind": "stop"
        }));
        let (def, p) = compile_stage(&styles, "room").expect("compiles");
        assert!(p.is_empty(), "bound tonemap clean: {p:?}");
        let (order, cyclic) = def.pass_order();
        assert!(!cyclic);
        let PassKind::TonemapGrade(t) = &def.recipe()[order[1]].kind else {
            panic!("the second pass is the tonemap_grade");
        };
        assert_eq!(
            t.grade,
            Vec3::new(1.18, 0.92, 0.68),
            "the TINT is authored art and survives beside the binds"
        );
        assert_eq!(
            t.binds,
            vec![
                (TonemapSlot::GradeStrength, "grade_warmth".to_string()),
                (TonemapSlot::Exposure, "stop".to_string()),
            ],
            "both bindable slots parsed, in roster order"
        );
        // And the bind is what the frame graph applies: the published warmth REPLACES the
        // (unauthored, defaulted-0) strength.
        let mut inputs = flicker_render::StageInputs::default();
        inputs.set("grade_warmth", 0.31).set("stop", 1.2);
        assert_eq!(
            t.resolve(&inputs),
            (Vec3::new(1.18, 0.92, 0.68), 0.31, 1.2),
            "the recipe resolves to the published numbers"
        );

        // A STRENGTH AUTHORED **AND** BOUND → dead-data problem (the shared binds() gate).
        let dead = hdr_stage(serde_json::json!({
            "pass": "tonemap_grade",
            "grade_strength": 0.4,
            "grade_strength_bind": "grade_warmth"
        }));
        let (_, p) = compile_stage(&dead, "room").expect("compiles");
        assert!(
            p.iter()
                .any(|m| m.contains("`grade_strength`") && m.contains("authored AND bound")),
            "a grade_strength authored and bound must be loud: {p:?}"
        );

        // A MISSPELLED bind key → problem (not a silently static grade).
        let typo = hdr_stage(serde_json::json!({
            "pass": "tonemap_grade",
            "grade_strength_bnid": "grade_warmth"
        }));
        let (_, p) = compile_stage(&typo, "room").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("`grade_strength_bnid`")),
            "a misspelled tonemap bind key must be named: {p:?}"
        );

        // A `$token` (an unresolved colour ref) is not a key a scene can publish.
        let token = hdr_stage(serde_json::json!({
            "pass": "tonemap_grade",
            "exposure_bind": "$warm"
        }));
        let (_, p) = compile_stage(&token, "room").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("`$warm`")),
            "a $token bind must be named: {p:?}"
        );

        // NO BINDS → the authored numbers stand (the solarbirth cinematic's static grade).
        let static_grade = hdr_stage(serde_json::json!({
            "pass": "tonemap_grade",
            "grade": [1.06, 1.0, 0.92, 1.0],
            "grade_strength": 0.12
        }));
        let (def, p) = compile_stage(&static_grade, "room").expect("compiles");
        assert!(p.is_empty(), "static tonemap clean: {p:?}");
        let (order, _) = def.pass_order();
        let PassKind::TonemapGrade(t) = &def.recipe()[order[1]].kind else {
            panic!("the second pass is the tonemap_grade");
        };
        assert!(t.binds.is_empty(), "nothing bound");
        assert_eq!(
            t.resolve(&flicker_render::StageInputs::default()),
            (Vec3::new(1.06, 1.0, 0.92), 0.12, 1.0),
            "no binds = the authored grade, unchanged"
        );
    }

    /// **GATE — the `bloom` parser: a synthetic HDR bloom stage compiles clean and orders
    /// correctly, and every misuse is a NAMED problem (rule 4BB12A75).** Bloom derives AFTER
    /// `scene` (it reads the `hdr` the scene writes) and BEFORE `tonemap_grade` (it writes the
    /// `hdr` the tonemap reads), carries its four art knobs, and a misspelled key, a negative
    /// threshold/intensity, and a bloom on a non-HDR surface each fail loud.
    #[test]
    fn the_bloom_pass_compiles_and_names_its_misuse() {
        let styles = serde_json::json!({
            "stages": {
                "lighting": { "day": { "sun_dir": [0.4, 0.8, 0.5], "sun": [1.0, 1.0, 1.0] } },
                "room": {
                    "lighting": "day",
                    "attachments": {
                        "color": {}, "depth": { "format": "depth32" },
                        "hdr": { "format": "rgba16f" }
                    },
                    "passes": [
                        { "pass": "scene", "writes": ["hdr", "depth"] },
                        { "pass": "bloom", "threshold": 1.2, "knee": 0.3, "intensity": 0.7, "radius": 2.0 },
                        { "pass": "tonemap_grade" }
                    ]
                }
            }
        });

        // CLEAN, and the derived order is scene → bloom → tonemap.
        let (def, p) = compile_stage(&styles, "room").expect("compiles");
        assert!(p.is_empty(), "bloom stage clean: {p:?}");
        let (order, cyclic) = def.pass_order();
        assert!(!cyclic);
        let kinds: Vec<&str> = order.iter().map(|&i| def.recipe()[i].kind.kind()).collect();
        assert_eq!(
            kinds,
            ["scene", "bloom", "tonemap_grade"],
            "bloom follows the scene (reads its hdr) and precedes the tonemap: {kinds:?}"
        );
        // The pass carried its authored knobs and reads/writes the hdr (the ordering channel).
        let PassKind::Bloom(b) = &def.recipe()[order[1]].kind else {
            panic!("the second pass is the bloom");
        };
        assert!((b.threshold - 1.2).abs() < 1e-6 && (b.knee - 0.3).abs() < 1e-6);
        assert!((b.intensity - 0.7).abs() < 1e-6 && (b.radius - 2.0).abs() < 1e-6);
        assert_eq!(
            def.recipe()[order[1]].reads,
            ["hdr"],
            "bloom reads the hdr it blooms"
        );
        assert_eq!(
            def.recipe()[order[1]].writes,
            ["hdr"],
            "bloom writes the hdr the tonemap then resolves"
        );

        // A MISSPELLED KEY → problem.
        let typo = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day",
                "attachments": { "color": {}, "depth": { "format": "depth32" }, "hdr": { "format": "rgba16f" } },
                "passes": [
                    { "pass": "scene", "writes": ["hdr", "depth"] },
                    { "pass": "bloom", "threshhold": 1.0 },
                    { "pass": "tonemap_grade" }
                ] } } });
        let (_, p) = compile_stage(&typo, "s").expect("compiles");
        assert!(
            p.iter().any(|m| m.contains("`threshhold`")),
            "a misspelled bloom key must be named: {p:?}"
        );

        // A NEGATIVE threshold / intensity → out-of-range problems (physically wrong picture).
        let neg = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day",
                "attachments": { "color": {}, "depth": { "format": "depth32" }, "hdr": { "format": "rgba16f" } },
                "passes": [
                    { "pass": "scene", "writes": ["hdr", "depth"] },
                    { "pass": "bloom", "threshold": -1.0, "intensity": -0.5 },
                    { "pass": "tonemap_grade" }
                ] } } });
        let (_, p) = compile_stage(&neg, "s").expect("compiles");
        assert!(
            p.iter()
                .any(|m| m.contains("`threshold`") && m.contains(">= 0")),
            "a negative threshold must be loud: {p:?}"
        );
        assert!(
            p.iter()
                .any(|m| m.contains("`intensity`") && m.contains(">= 0")),
            "a negative intensity must be loud: {p:?}"
        );

        // A BLOOM on a surface with NO hdr attachment → half-wired problem (it reads nothing).
        let no_hdr = serde_json::json!({ "stages": {
            "lighting": { "day": { "sun_dir": [0.0, 1.0, 0.0], "sun": [1.0, 1.0, 1.0] } },
            "s": { "lighting": "day", "passes": [
                { "pass": "scene" },
                { "pass": "bloom" }
            ] } } });
        let (_, p) = compile_stage(&no_hdr, "s").expect("compiles");
        assert!(
            p.iter()
                .any(|m| m.contains("`bloom`") && m.contains("rgba16f")),
            "a bloom with no hdr attachment must be loud: {p:?}"
        );
    }

    /// **GATE — every `source` a shipped surface names resolves.** The walker only
    /// WARNS at runtime for a surface whose source is missing (and still reserves the
    /// rect, so the panel reads as empty); a scene file shipping that is a build failure
    /// here, named by scene and node.
    #[test]
    fn every_surface_source_in_a_shipped_scene_resolves() {
        let mut named = 0;
        for (id, def, styles) in shipped_scenes() {
            let Some(tree) = &def.tree else { continue };
            walk(tree, &mut |n| {
                if n.component != "surface" {
                    return;
                }
                if let Some(source) = source_of(n) {
                    named += 1;
                    assert!(
                        compile_stage(&styles, source).is_some(),
                        "{id}.scene.json: surface `{}` names stage `{source}`, which neither \
                         the scene's own `stages` nor the shared library authors",
                        n.id
                    );
                }
            });
        }
        assert!(named > 0, "shipped surfaces name sources");
        // A shell fragment has no scene file of its own, so a source it names can only
        // come from the shared library.
        let shared = shipped_styles();
        for (file, id, source) in shared_fragment_sources() {
            assert!(
                compile_stage(&shared, &source).is_some(),
                "shared/{file}: surface `{id}` names stage `{source}`, which the shared \
                 library does not author"
            );
        }
    }

    /// **GATE — a stage in the shared library is SHARED.** A source that only one
    /// scene's surfaces name is that scene's data and belongs in its `.scene.json`
    /// (ruling 8DE71FB0); the library holds what more than one scene draws from.
    #[test]
    fn a_stage_in_the_shared_library_is_shared() {
        let styles = shipped_styles();
        let library: Vec<String> = styles["stages"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| is_source_key(k))
            .cloned()
            .collect();
        let scenes = shipped_scenes();
        for source in library {
            let users: Vec<&str> = scenes
                .iter()
                .filter(|(_, def, _)| {
                    let mut found = false;
                    if let Some(tree) = &def.tree {
                        walk(tree, &mut |n| {
                            found |= n.component == "surface" && source_of(n) == Some(&source)
                        });
                    }
                    found
                })
                .map(|(id, _, _)| id.as_str())
                .collect();
            assert!(
                users.len() >= 2,
                "ui_stages.json authors `{source}`, which {} scene(s) name ({users:?}) — a \
                 stage one scene uses lives in that scene's .scene.json `stages` section",
                users.len()
            );
        }
    }

    /// A scene's own `stages` merge INTO the shared block beside the presets (never
    /// over it), a scene stage may not take a library name, and the fold from the
    /// scene file's top-level section is what the loader sees.
    #[test]
    fn a_scenes_stages_merge_into_the_shared_block_and_never_shadow_the_library() {
        let shared = serde_json::json!({
            "theme": { "tokens": { "void": [0.0, 0.0, 0.0, 0.0] } },
            "stages": {
                "lighting": { "studio": { "sun": [1.0, 1.0, 1.0] } },
                "library_doll": { "layers": [{ "draw": "skinned" }] }
            }
        });
        let scene = crate::SceneDef::parse(
            "bench",
            r#"{ "behaviour": "bench",
                 "stages": {
                   "bench_globe": { "lighting": "studio", "clear": "$void",
                                    "layers": [{ "draw": "shells" }] },
                   "library_doll": { "layers": [{ "draw": "material" }] }
                 } }"#,
        )
        .expect("a scene file with a stages section parses");
        assert!(
            scene.stages().is_some_and(|s| s.len() == 2),
            "folded under styles"
        );
        let styles = crate::load_styles_strs_for(&[&shared.to_string()], scene.styles.as_ref());
        let stages = styles["stages"].as_object().unwrap();
        assert!(
            stages.contains_key("lighting"),
            "the presets survive the merge"
        );
        let globe = stage_def(&styles, "bench_globe").expect("the scene's stage landed");
        assert_eq!(
            globe.clear,
            Some([0.0; 4]),
            "its $token resolved against the palette"
        );
        assert!(
            (globe.lighting.sky_sun().color.x - 1.0).abs() < 1e-6,
            "and it found the preset"
        );
        // The library's definition won the collision.
        assert!(stage_def(&styles, "library_doll")
            .unwrap()
            .has_layer("skinned"));

        // One spelling: stages under `styles`, or a preset inside a scene, is refused.
        assert!(crate::SceneDef::parse(
            "bad",
            r#"{ "behaviour": "b", "styles": { "stages": {} } }"#
        )
        .is_err());
        assert!(crate::SceneDef::parse(
            "bad",
            r#"{ "behaviour": "b", "stages": { "lighting": {} } }"#
        )
        .is_err());
    }

    /// The whole vocabulary compiles through one reader, and everything a reader used
    /// to shrug at is now a NAMED problem: an unknown draw kind, a misspelled key, an
    /// unauthored preset, a camera kind that is not orbit, a token the palette lacks.
    #[test]
    fn the_whole_vocabulary_compiles_and_every_unknown_is_a_problem() {
        let styles = serde_json::json!({
            "stages": {
                "_comment": "not a source",
                "lighting": {
                    "studio": { "sun_dir": [0.4, 0.8, 0.5], "sun": [0.85, 0.85, 0.85],
                                "ambient": [0.35, 0.35, 0.35],
                                "sky_zenith": [0.1, 0.2, 0.3], "bogus": 1 }
                },
                "all": {
                    "lighting": "studio",
                    "clear": [0.0, 0.0, 0.0, 0.0],
                    "camera": { "kind": "orbit", "yaw": 0.5, "pitch": 0.2, "dist": 3.0, "target_y": 1.0 },
                    "layers": [
                        { "draw": "skinned" },
                        { "draw": "ring", "radius": 0.5, "segments": 12, "color": [1.0, 0.5, 0.0, 1.0] },
                        { "draw": "grid", "spacing": 1.0, "extent": 4.0 },
                        { "draw": "shells" },
                        { "draw": "shell", "radius_scale": 0.9, "inset": 0.1, "color": [0.1, 0.2, 0.3, 1.0] },
                        { "draw": "graticule", "radius_scale": 1.02 },
                        { "draw": "material" }
                    ]
                },
                "sloppy": {
                    "lighting": "noon",
                    "clear": "$no_such_token",
                    "camera": { "kind": "dolly", "yaww": 1.0 },
                    "layers": [
                        { "draw": "ring", "radiuss": 1.0 },
                        { "draw": "unheard_of" },
                        { "no_draw_key": true }
                    ],
                    "extra": true
                }
            }
        });
        let (all, problems) = compile_stage(&styles, "all").unwrap();
        // The preset's stray key is the only complaint on the clean source.
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("stages.lighting.studio.bogus"));
        assert_eq!(all.layers.len(), 7, "every kind the engine knows");
        assert_eq!(
            all.layers.iter().map(StageLayer::kind).collect::<Vec<_>>(),
            StageLayer::KINDS
        );
        assert!(
            (all.lighting.sky_zenith.z - 0.3).abs() < 1e-6,
            "the sky palette rides the preset"
        );
        let cam = all.camera.expect("authored framing");
        assert!((cam.dist - 3.0).abs() < 1e-6 && (cam.target_y - 1.0).abs() < 1e-6);
        assert!(matches!(
            all.layers[1],
            StageLayer::Ring { segments: 12, .. }
        ));
        assert!(
            matches!(all.layers[4], StageLayer::Shell { color: [x, _, _], .. } if (x - 0.1).abs() < 1e-6)
        );

        let (sloppy, problems) = compile_stage(&styles, "sloppy").unwrap();
        let text = problems.join("\n");
        for expected in [
            "lighting names `noon`",
            "clear names `$no_such_token`",
            "camera.kind must be \"orbit\"",
            "camera.yaww is not a camera key",
            "layers[0] (ring) has no key `radiuss`",
            "layers[1] draws `unheard_of`",
            "layers[2] has no `draw` kind",
            "sloppy.extra is not a stage key",
        ] {
            assert!(
                text.contains(expected),
                "missing problem `{expected}` in:\n{text}"
            );
        }
        // …and every one of those still degraded to a usable value, never a panic.
        assert_eq!(sloppy.layers.len(), 1, "only the ring survived");
        assert!(sloppy.camera.unwrap().dist > 0.0);
        assert_eq!(
            sloppy.clear, None,
            "a clear whose token never resolved is UNAUTHORED, not black"
        );
        assert!(
            sloppy.lighting.sky_sun().color.x > 0.0,
            "the default light is lit"
        );

        assert!(compile_stage(&styles, "_comment").is_some_and(|(_, p)| !p.is_empty()));
        assert!(compile_stage(&styles, "missing").is_none());
        assert!(stage_defs(&styles).keys().all(|k| is_source_key(k)));
        assert_eq!(stage_defs(&styles).len(), 2);
    }

    /// Authored JSON reaches the compiler unvalidated; malformed VALUES degrade to
    /// defaults (with a problem each) rather than panicking or emitting NaN geometry.
    #[test]
    fn malformed_values_degrade_to_defaults_with_a_problem_each() {
        let styles = serde_json::json!({
            "stages": {
                "broken": {
                    "camera": { "dist": "nonsense", "yaw": null },
                    "layers": [
                        { "draw": "ring", "radius": -1.0, "segments": 24, "color": [1, 2, 3] },
                        { "draw": "grid", "spacing": "wide" }
                    ]
                }
            }
        });
        let (b, problems) = compile_stage(&styles, "broken").unwrap();
        let cam = b.camera.unwrap();
        assert!(
            cam.dist.is_finite() && cam.dist > 0.0,
            "bad dist falls back"
        );
        assert!(cam.yaw.is_finite());
        assert_eq!(b.layers.len(), 2);
        assert!(matches!(
            b.layers[0],
            StageLayer::Ring {
                color: RING_COLOR,
                ..
            }
        ));
        assert!(matches!(b.layers[1], StageLayer::Grid { spacing, .. } if spacing == 0.5));
        assert_eq!(problems.len(), 4, "{problems:?}");
        // A negative radius is a GEOMETRY guard's job (ring_segments yields nothing); the
        // compiler carries the authored number through.
        assert!(matches!(b.layers[0], StageLayer::Ring { radius, .. } if radius == -1.0));
    }

    /// **The recipe vocabulary compiles, and every unresolved name in it is a NAMED
    /// problem.** A pass may only read and write images its own stage declares; a kind,
    /// a param, a bind or a rate the engine does not know is reported (never silently
    /// dropped); a number authored on a slot a `*_bind` already drives is reported as the
    /// dead data it is; an `rgba16f` attachment with no `tonemap_grade` to resolve it is a
    /// problem; and the executed ORDER is derived from reads and writes, so no file anywhere
    /// spells a pass number — a fog-BEFORE-disk recipe is now simply that order, not a
    /// refusal (`encode_passes` honours the recipe order).
    #[test]
    fn a_recipe_names_only_declared_attachments_and_orders_itself() {
        let styles = serde_json::json!({
            "stages": {
                "lighting": { "deep_space": { "sun": [0.0, 0.0, 0.0], "ambient": [0.02, 0.02, 0.03],
                                              "point_pos": [0.0, 0.0, 0.0], "point": [1.0, 0.86, 0.66] } },
                "clean": {
                    "lighting": "deep_space",
                    "attachments": {
                        "_comment": "not an image",
                        "color": { "format": "surface" },
                        "depth": { "format": "depth32", "scale": 1.0 }
                    },
                    "rate": "live",
                    "passes": [
                        { "pass": "volumetric_disk", "reads": ["depth"], "writes": ["color"],
                          "inner": 0.35, "outer": 21.0, "snow_line": 4.6, "scale_height": 0.10,
                          "density": 3.5, "tint": [0.038, 0.033, 0.052, 1.0],
                          "glow": [0.85, 0.44, 0.22, 1.0],
                          "formation_bind": "dust_formation", "time_bind": "dust_time" },
                        { "pass": "sky" },
                        { "pass": "scene" }
                    ]
                },
                "sloppy": {
                    "attachments": { "color": { "format": "rgba16f", "scale": -1.0, "formats": 1 },
                                     "glow": "not an object" },
                    "rate": { "hz": -1.0 },
                    "passes": [
                        { "pass": "ground_fog", "bottom": -2.0, "top": 12.0, "flooor": 3.0,
                          "wind": [0.4], "floor_bind": "$fog_floor" },
                        { "pass": "volumetric_disk", "reads": ["glow", "depth"],
                          "formation": 0.5, "formation_bind": "dust_formation" },
                        { "pass": "smoke" },
                        { "no_pass_key": true }
                    ]
                },
                // Authored disk-FIRST, but its `reads` puts it after a fog that writes
                // what it reads — the executed order is the one the refusal inspects.
                "reordered": {
                    "attachments": {
                        "color": { "format": "surface" },
                        "depth": { "format": "depth32" },
                        "haze": { "format": "surface" }
                    },
                    "passes": [
                        { "pass": "volumetric_disk", "reads": ["haze"] },
                        { "pass": "scene" },
                        { "pass": "ground_fog", "writes": ["haze"] }
                    ]
                }
            }
        });

        let (clean, problems) = compile_stage(&styles, "clean").unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(clean.rate, Rate::Live);
        // Authored disk-first, executed depth-writer-first: the order is DERIVED.
        let (order, cyclic) = clean.pass_order();
        assert!(!cyclic);
        assert_eq!(
            order
                .iter()
                .map(|&i| clean.recipe()[i].kind.kind())
                .collect::<Vec<_>>(),
            ["sky", "scene", "volumetric_disk"],
            "a reader of `depth` lands after the pass that writes it; the two \
             colour-only writers keep authored order"
        );
        let PassKind::VolumetricDisk(disk) = &clean.passes[0].kind else {
            panic!("the disk compiled")
        };
        assert!((disk.disk.snow_line - 4.6).abs() < 1e-6);
        assert!(
            (disk.disk.glow.x - 0.85).abs() < 1e-6,
            "rgba truncated to rgb"
        );
        assert_eq!(
            disk.binds,
            vec![
                (VolumetricSlot::Formation, "dust_formation".to_string()),
                (VolumetricSlot::Time, "dust_time".to_string()),
            ]
        );
        assert!(disk.disk.gaps.is_empty(), "gaps are never authored");
        // The preset's point light is the half `deep_space` needs and nothing authored
        // before it could say.
        assert!(
            clean.lighting.lights[2].color.length() > 0.5
                && clean.lighting.lights[2].kind == LightKind::Point
                && clean.lighting.sky_sun().color == Vec3::ZERO
        );

        let (sloppy, problems) = compile_stage(&styles, "sloppy").unwrap();
        let text = problems.join("\n");
        for expected in [
            // The rgba16f `color` now RENDERS — but nothing tonemaps it, so it is the
            // half-wired surface the hdr⟺tonemap gate names (the old "lands in S3" refusal
            // is gone).
            "declares an rgba16f attachment but no `tonemap_grade`",
            "attachments.color.scale must be greater than zero",
            "attachments.color.formats is not an attachment key",
            "attachments.glow must be an object",
            "rate.hz must be greater than zero",
            "passes[0] (ground_fog) has no key `flooor`",
            "passes[0].wind must be 2 finite numbers",
            "passes[0].floor_bind names `$fog_floor`",
            "passes[1].reads[1] names `depth`, which this stage's attachments do not declare",
            "passes[1] `formation` is authored AND bound by `formation_bind`",
            "passes[2] runs `smoke`",
            "passes[3] has no `pass` kind",
            // Fog-before-disk is no longer refused — that assertion is gone deliberately.
            "runs no `scene` pass",
        ] {
            assert!(
                text.contains(expected),
                "missing problem `{expected}` in:\n{text}"
            );
        }
        // …and every one of those degraded to a usable value rather than a panic: the
        // two readable passes survived, the unusable rate is live, and the read that
        // named an image this stage does not own is simply not an edge (this stage
        // declared `glow` and never declared `depth`, so its disk reads `glow` alone).
        assert_eq!(
            sloppy
                .passes
                .iter()
                .map(|p| p.kind.kind())
                .collect::<Vec<_>>(),
            ["ground_fog", "volumetric_disk"]
        );
        assert_eq!(sloppy.rate, Rate::Live);
        assert_eq!(sloppy.passes[1].reads, ["glow"]);
        // The bind still landed — the dead number beside it is a problem, not a refusal.
        let PassKind::VolumetricDisk(disk) = &sloppy.passes[1].kind else {
            panic!("the disk compiled")
        };
        assert_eq!(
            disk.binds,
            vec![(VolumetricSlot::Formation, "dust_formation".to_string())]
        );

        // The order is DERIVED from reads/writes: this recipe declares the disk FIRST and
        // then makes it read what the fog writes, so the fog runs in front of it. That used
        // to be refused; now it simply IS the executed order the split encoder honours, so
        // the recipe compiles CLEAN.
        let (reordered, problems) = compile_stage(&styles, "reordered").unwrap();
        let (order, cyclic) = reordered.pass_order();
        assert!(!cyclic);
        assert_eq!(
            order
                .iter()
                .map(|&i| reordered.recipe()[i].kind.kind())
                .collect::<Vec<_>>(),
            ["scene", "ground_fog", "volumetric_disk"],
            "the fog reordered itself in front of the disk through reads/writes"
        );
        assert!(
            problems.is_empty(),
            "fog-before-disk is a legal order now, not a refusal: {problems:?}"
        );
        // An unauthored `attachments` block is the colour+depth pair every surface has
        // always had, and an unauthored recipe is the one content pass.
        let plain = StageDef::default();
        assert_eq!(plain.attachments, Attachments::default());
        assert_eq!(plain.recipe().len(), 1);
    }

    /// **An HDR surface compiles clean and tonemaps LAST — and every way of authoring one
    /// wrong is NAMED.** A stage that declares an `hdr` (rgba16f) attachment, repoints the
    /// lit passes' `writes` at it, and appends a `tonemap_grade` pass is the S3b shape: it
    /// compiles with no problems, its pass-owned grade params land, and the tonemap derives
    /// LAST (it reads `hdr`, which the lit passes write).
    ///
    /// The three ways that shape can be authored into something the renderer cannot honour
    /// each fail loud here rather than degrade: a SCALED `hdr` (the resolve is 1:1 and
    /// `Attachments::pixels` sizes every image off `color`'s scale, so the number does
    /// nothing), an `hdr` in any format but `rgba16f` (the allocation takes the declared
    /// format, so anything else is silently wrong on the GPU), and a `tonemap_grade` whose
    /// `reads` drops `hdr` (that read IS its position in the derived order).
    #[test]
    fn an_hdr_surface_tonemaps_last_and_compiles_clean() {
        // A scaled `hdr` — a half-resolution HDR nothing can honour.
        let scaled = serde_json::json!({
            "stages": { "s": {
                "attachments": { "hdr": { "format": "rgba16f", "scale": 0.5 },
                                 "color": { "format": "surface" },
                                 "depth": { "format": "depth32" } },
                "passes": [ { "pass": "scene", "writes": ["hdr", "depth"] },
                            { "pass": "tonemap_grade" } ]
            } }
        });
        let (_, problems) = compile_stage(&scaled, "s").unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("attachments.hdr.scale") && p.contains("must be 1.0")),
            "a scaled hdr must be named: {problems:?}"
        );

        // An `hdr` that is not rgba16f — a format the tonemap cannot read.
        let wrong_format = serde_json::json!({
            "stages": { "s": {
                "attachments": { "hdr": { "format": "surface" },
                                 "color": { "format": "surface" },
                                 "depth": { "format": "depth32" } },
                "passes": [ { "pass": "scene", "writes": ["hdr", "depth"] },
                            { "pass": "tonemap_grade" } ]
            } }
        });
        let (_, problems) = compile_stage(&wrong_format, "s").unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("attachments.hdr.format") && p.contains("rgba16f")),
            "an hdr in the wrong format must be named: {problems:?}"
        );

        // A tonemap that does not read `hdr` — its position would be decoration.
        let unread = serde_json::json!({
            "stages": { "s": {
                "attachments": { "hdr": { "format": "rgba16f" },
                                 "color": { "format": "surface" },
                                 "depth": { "format": "depth32" } },
                "passes": [ { "pass": "scene", "writes": ["hdr", "depth"] },
                            { "pass": "tonemap_grade", "reads": ["depth"] } ]
            } }
        });
        let (_, problems) = compile_stage(&unread, "s").unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("tonemap_grade") && p.contains("must READ `hdr`")),
            "a tonemap that does not read hdr must be named: {problems:?}"
        );

        let styles = serde_json::json!({
            "stages": {
                "lighting": { "lit": { "sun": [1.0, 0.98, 0.95], "ambient": [0.15, 0.15, 0.18] } },
                "hdr_world": {
                    "lighting": "lit",
                    "attachments": {
                        "hdr": { "format": "rgba16f" },
                        "color": { "format": "surface" },
                        "depth": { "format": "depth32" }
                    },
                    "passes": [
                        { "pass": "sky", "writes": ["hdr"] },
                        { "pass": "scene", "writes": ["hdr", "depth"] },
                        { "pass": "tonemap_grade", "grade": [1.0, 0.92, 0.82, 1.0],
                          "grade_strength": 0.12, "exposure": 1.15 }
                    ]
                }
            }
        });
        let (def, problems) = compile_stage(&styles, "hdr_world").unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        let (order, cyclic) = def.pass_order();
        assert!(!cyclic);
        let kinds: Vec<&str> = order.iter().map(|&i| def.recipe()[i].kind.kind()).collect();
        assert_eq!(
            kinds.last(),
            Some(&"tonemap_grade"),
            "the tonemap resolves the HDR colour last: {kinds:?}"
        );
        let PassKind::TonemapGrade(t) = &def.recipe()[*order.last().unwrap()].kind else {
            panic!("the tonemap compiled")
        };
        assert!((t.exposure - 1.15).abs() < 1e-6, "exposure landed");
        assert!((t.grade_strength - 0.12).abs() < 1e-6, "strength landed");
        assert!(
            (t.grade.x - 1.0).abs() < 1e-6 && (t.grade.y - 0.92).abs() < 1e-6,
            "grade tint truncated rgba→rgb"
        );
    }

    /// **An HDR attachment and its tonemap are mutually required.** An `rgba16f` attachment
    /// with no `tonemap_grade` to resolve it, and a `tonemap_grade` with no `rgba16f` to
    /// feed it, are each a NAMED problem — the fail-loud coupling that keeps a half-wired
    /// HDR surface out of the shipped corpus (rule 4BB12A75). This is the gate that replaced
    /// the blanket "rgba16f lands in S3" refusal.
    #[test]
    fn hdr_attachment_and_tonemap_are_mutually_required() {
        // rgba16f attachment, no tonemap → a source nothing resolves.
        let no_tonemap = serde_json::json!({
            "stages": { "s": {
                "attachments": { "hdr": { "format": "rgba16f" },
                                 "color": { "format": "surface" },
                                 "depth": { "format": "depth32" } },
                "passes": [ { "pass": "scene", "writes": ["hdr", "depth"] } ]
            } }
        });
        let (_, problems) = compile_stage(&no_tonemap, "s").unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("rgba16f attachment") && p.contains("tonemap_grade")),
            "an rgba16f with no tonemap must be named: {problems:?}"
        );

        // tonemap, no rgba16f → a resolve with nothing to tonemap.
        let no_hdr = serde_json::json!({
            "stages": { "s": {
                "attachments": { "color": { "format": "surface" },
                                 "depth": { "format": "depth32" } },
                "passes": [ { "pass": "scene" }, { "pass": "tonemap_grade" } ]
            } }
        });
        let (_, problems) = compile_stage(&no_hdr, "s").unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("tonemap_grade") && p.contains("no rgba16f attachment feeds")),
            "a tonemap with no hdr source must be named: {problems:?}"
        );
    }

    /// **GATE — every lit-3D shipped stage resolves through EXACTLY ONE tonemap.** The S3b
    /// content flip took every lit-3D surface HDR: its lit passes render into an rgba16f
    /// `hdr` working attachment and one `tonemap_grade` resolves that back into the surface
    /// `color`. This gate proves the property over the REAL shipped corpus (the same stages
    /// `every_shipped_stage_compiles_clean` walks): a stage that declares an rgba16f `hdr`
    /// attachment has EXACTLY one `tonemap_grade`, that pass READS `hdr`, and `pass_order`
    /// derives it LAST (it reads what the lit passes write). It also proves the flip is
    /// PRESENT — the seven cinematic / globe / portrait / material stages are each found on
    /// the HDR path. The compile-clean gate alone would still pass if a flip were silently
    /// dropped (it only checks that what IS authored is clean), so THIS is the gate that
    /// fails loud if a shipped stage falls back off HDR (rule 4BB12A75).
    #[test]
    fn every_lit3d_shipped_stage_resolves_through_exactly_one_tonemap() {
        /// Assert one stage's HDR resolve is well-formed; return whether it is HDR at all.
        fn hdr_resolved(styles: &Json, source: &str, where_: &str) -> bool {
            let (def, problems) = compile_stage(styles, source)
                .unwrap_or_else(|| panic!("{where_}: `{source}` exists"));
            assert!(
                problems.is_empty(),
                "{where_} stages.{source}: {problems:?}"
            );
            let hdr_names: Vec<&str> = def
                .attachments
                .names()
                .filter(|&n| {
                    def.attachments
                        .get(n)
                        .is_some_and(|a| a.format == AttachmentFormat::Rgba16f)
                })
                .collect();
            if hdr_names.is_empty() {
                return false; // not a lit-3D HDR surface — nothing to resolve.
            }
            assert_eq!(
                hdr_names,
                [Attachments::HDR],
                "{where_} stages.{source}: an HDR surface's rgba16f attachment is the `hdr` one"
            );
            let tonemaps = def
                .recipe()
                .iter()
                .filter(|p| matches!(p.kind, PassKind::TonemapGrade(_)))
                .count();
            assert_eq!(
                tonemaps, 1,
                "{where_} stages.{source}: an HDR surface resolves through EXACTLY one \
                 tonemap_grade (found {tonemaps})"
            );
            let tm = def
                .recipe()
                .iter()
                .find(|p| matches!(p.kind, PassKind::TonemapGrade(_)))
                .unwrap();
            assert!(
                tm.reads.iter().any(|r| r.as_str() == Attachments::HDR),
                "{where_} stages.{source}: the tonemap_grade must READ the `hdr` attachment"
            );
            let (order, cyclic) = def.pass_order();
            assert!(
                !cyclic,
                "{where_} stages.{source}: the recipe order is cyclic"
            );
            let last = *order.last().expect("a recipe has at least one pass");
            assert!(
                matches!(def.recipe()[last].kind, PassKind::TonemapGrade(_)),
                "{where_} stages.{source}: the tonemap_grade resolves LAST (it reads what \
                 the lit passes write)"
            );
            true
        }

        let mut hdr_stages: Vec<String> = Vec::new();
        let styles = shipped_styles();
        let stages = styles["stages"]
            .as_object()
            .expect("the shipped stages block");
        for source in stages.keys().filter(|k| is_source_key(k)) {
            if hdr_resolved(&styles, source, "ui_stages.json") {
                hdr_stages.push(source.clone());
            }
        }
        for (id, def, styles) in shipped_scenes() {
            for source in def.stages().into_iter().flat_map(|s| s.keys()) {
                if hdr_resolved(&styles, source, &format!("{id}.scene.json")) {
                    hdr_stages.push(source.clone());
                }
            }
        }

        // The flip is PRESENT: every lit-3D stage S3b took HDR is found on the path. Absent
        // this, a silent revert to the non-HDR encode would pass every compile-clean gate.
        // (`pocepochs_globe` + `godmode_globe` left the corpus with their retired scenes,
        // 2026-08-26 — superseded by the Populous Bench.)
        for expected in [
            "solarbirth_sky",
            "pocclusters_world",
            "populous_globe",
            "sablework_lit",
            "portrait",
        ] {
            assert!(
                hdr_stages.iter().any(|s| s.as_str() == expected),
                "the shipped corpus no longer resolves `{expected}` through HDR + \
                 tonemap_grade — found {hdr_stages:?}"
            );
        }
        assert!(
            hdr_stages.len() >= 5,
            "expected at least the five surviving S3b lit-3D stages on the HDR path, found \
             {hdr_stages:?}"
        );
    }

    /// A globe stage authors NO camera and that absence is a decision — the compiled
    /// definition says so (`None`), and a stage that authors one gets the portrait
    /// defaults for whatever it leaves out. Its `clear` is the same shape of decision:
    /// unauthored is `None`, which the frame graph reads as "transparent offscreen, the
    /// window's own colour on screen" — never a black it invented.
    #[test]
    fn the_camera_is_optional_and_partial_framings_fill_from_the_portrait() {
        let styles = serde_json::json!({
            "stages": {
                "globe": { "layers": [{ "draw": "shells" }] },
                "framed": { "camera": { "dist": 4.0 } }
            }
        });
        assert!(stage_def(&styles, "globe").unwrap().camera.is_none());
        assert!(stage_def(&styles, "globe").unwrap().clear.is_none());
        let cam = stage_def(&styles, "framed").unwrap().camera.unwrap();
        assert!((cam.dist - 4.0).abs() < 1e-6);
        assert!((cam.yaw - StageCamera::default().yaw).abs() < 1e-6);
        assert!(lighting_preset(&styles, "studio").is_none());
    }
    // ---------------------------------------------------------------------------
    // The light-rig gates (S4a). The rig replaced the sun/moon/point triple; these
    // prove the SHIPPED content still compiles to exactly that triple, and that the
    // shader's new count-bounded loop computes bit-for-bit what the closed form did.
    // ---------------------------------------------------------------------------

    /// **GATE — every shipped preset compiles to a RIG with no problems.** The legacy
    /// keys must land in slots 0/1/2 as Dir sun · Dir moon · Point, black ones included,
    /// with `count == 3` — that fixed order is what `sky_sun()`/`sky_moon()` (SLOTS 0
    /// and 1), the celestial cycle and the identity gate below all read through. A preset in the
    /// GENERAL form (`hearth`) is checked against its own array instead: the roster is
    /// what it authored, in the order it authored it — the slot order the celestial
    /// cycle writes sun/moon by index into.
    #[test]
    fn every_shipped_preset_compiles_to_a_rig_with_no_problems() {
        let styles = shipped_styles();
        let table = styles["stages"]["lighting"]
            .as_object()
            .expect("the preset table");
        let (mut seen, mut general) = (0, 0);
        for (name, raw) in table.iter().filter(|(n, _)| !n.starts_with('_')) {
            let mut problems = Vec::new();
            let rig = compile_preset(&styles, name, &mut problems).expect("preset exists");
            assert!(problems.is_empty(), "stages.lighting.{name}: {problems:?}");
            // The general form: the authored array IS the roster, entry for entry.
            if let Some(entries) = raw.get("lights") {
                let entries = entries
                    .as_array()
                    .unwrap_or_else(|| panic!("stages.lighting.{name}.lights is an array"));
                assert_eq!(
                    rig.count as usize,
                    entries.len(),
                    "stages.lighting.{name}: every authored light compiles (none dropped)"
                );
                for (i, e) in entries.iter().enumerate() {
                    let authored = e["kind"].as_str().unwrap_or("<missing>");
                    let kind = match authored {
                        "dir" => LightKind::Dir,
                        "point" => LightKind::Point,
                        "spot" => LightKind::Spot,
                        other => panic!("stages.lighting.{name}.lights[{i}].kind = `{other}`"),
                    };
                    assert_eq!(
                        rig.lights[i].kind, kind,
                        "stages.lighting.{name}.lights[{i}] keeps its authored slot"
                    );
                }
                general += 1;
                continue;
            }
            assert_eq!(
                rig.count, 3,
                "stages.lighting.{name} authors the legacy trio, so it compiles to 3 lights"
            );
            assert_eq!(
                rig.lights[0].kind,
                LightKind::Dir,
                "{name}: slot 0 is the sun"
            );
            assert_eq!(
                rig.lights[1].kind,
                LightKind::Dir,
                "{name}: slot 1 is the moon"
            );
            assert_eq!(
                rig.lights[2].kind,
                LightKind::Point,
                "{name}: slot 2 is the point light"
            );
            for (i, l) in rig.lights[..3].iter().enumerate() {
                assert_eq!(
                    l.intensity, 1.0,
                    "{name}[{i}]: colour carries the magnitude"
                );
                assert_eq!(l.radius, 0.0, "{name}[{i}]: no falloff, exactly as before");
                assert!(
                    l.driver.is_none(),
                    "{name}[{i}]: no shipped preset is driven"
                );
            }
            // The AUTHORED numbers themselves land in those slots.
            let authored = |key: &str| raw.get(key).map(|v| Vec3::new(f(v, 0), f(v, 1), f(v, 2)));
            if let Some(c) = authored("sun") {
                assert_eq!(rig.lights[0].color, c, "{name}: `sun` is slot 0's colour");
            }
            if let Some(c) = authored("moon") {
                assert_eq!(rig.lights[1].color, c, "{name}: `moon` is slot 1's colour");
            }
            if let Some(c) = authored("point") {
                assert_eq!(rig.lights[2].color, c, "{name}: `point` is slot 2's colour");
            }
            if let Some(p) = authored("point_pos") {
                assert_eq!(rig.lights[2].position, p, "{name}: `point_pos` is slot 2's");
            }
            if let Some(d) = authored("sun_dir") {
                assert_eq!(
                    rig.lights[0].direction,
                    d.normalize_or_zero(),
                    "{name}: `sun_dir` is slot 0's direction"
                );
            }
            if let Some(d) = authored("moon_dir") {
                assert_eq!(
                    rig.lights[1].direction,
                    d.normalize_or_zero(),
                    "{name}: `moon_dir` is slot 1's direction"
                );
            }
            seen += 1;
        }
        assert!(seen >= 3, "the library ships studio / night / deep_space");
        assert!(general >= 1, "…and `hearth`, in the general `lights` form");

        // **THE HEARTH** — the fireplace-class rig, checked as the SHAPE the Prism Test
        // Room's `Celestial::over` depends on: two reserved directional slots FIRST,
        // then the fire, so a cycle writing slots 0 and 1 by index can never land on it.
        let mut problems = Vec::new();
        let hearth = compile_preset(&styles, "hearth", &mut problems).expect("`hearth` is shipped");
        assert!(problems.is_empty(), "stages.lighting.hearth: {problems:?}");
        assert_eq!(
            hearth.count, 3,
            "hearth = the sun slot, the moon slot, and the fire"
        );
        assert_eq!(
            hearth.lights[..3]
                .iter()
                .map(|l| l.kind)
                .collect::<Vec<_>>(),
            [LightKind::Dir, LightKind::Dir, LightKind::Point],
            "the two directional slots come FIRST — the celestial cycle writes 0/1 by index"
        );
        for i in [0, 1] {
            assert_eq!(
                hearth.lights[i].color,
                Vec3::ZERO,
                "hearth slot {i} is RESERVED for the cycle, so it is authored black"
            );
            assert!(
                hearth.lights[i].driver.is_none(),
                "hearth slot {i} is overwritten every frame — a driver there is dead data"
            );
        }
        // ONE addressing scheme: the sky reads the same two SLOTS the cycle writes.
        assert_eq!(hearth.sky_sun(), hearth.lights[0], "the sky reads slot 0");
        assert_eq!(hearth.sky_moon(), hearth.lights[1], "and slot 1");

        let fire = hearth.lights[2];
        assert!(
            fire.radius > 0.0,
            "the fire is the first shipped light with real falloff"
        );
        assert!(
            fire.intensity > 1.0,
            "…which is exactly what makes `intensity` mean something: a hearth's is in \
             the tens, not the legacy 1.0 whose colour carried the magnitude ({})",
            fire.intensity
        );
        assert!(
            fire.color.x > fire.color.y && fire.color.y > fire.color.z,
            "firelight is warm — r > g > b ({:?})",
            fire.color
        );
        let driver = fire.driver.expect("the fire is DRIVEN");
        assert_eq!(driver.kind, DriverKind::Flicker, "a fire flickers");
        assert!(
            driver.depth > 0.0 && driver.speed > 0.0,
            "…and actually moves"
        );
        // Over a second of stage clock the gain must vary and must never exceed 1.0: a
        // fire dims, it never overshoots the radiance the author set.
        let mut varied = 0;
        for i in 0..64 {
            let g = driver.gain(i as f32 / 64.0);
            assert!(g > 0.0 && g <= 1.0, "flicker gain {g} left (0, 1]");
            if g < 0.999 {
                varied += 1;
            }
        }
        assert!(
            varied > 16,
            "the flicker actually modulates the fire ({varied}/64 samples dimmed)"
        );
    }

    fn f(v: &Json, i: usize) -> f32 {
        v[i].as_f64().unwrap() as f32
    }

    /// A CPU mirror of the shaders' `light_sample()`: `(unit vector toward the light,
    /// attenuation)`. Kept term-for-term with the WGSL in `mesh.wgsl` — which is a
    /// second source of truth, so the shipped text is gated against these terms by
    /// `flicker-render`'s `the_frame_prelude_is_one_text` and its light-loop sibling.
    fn light_sample(l: &Light, wp: Vec3) -> (Vec3, f32) {
        if l.kind == LightKind::Dir {
            return (l.direction, 1.0);
        }
        let to_l = l.position - wp;
        let dir = to_l / to_l.length().max(1e-4);
        let mut atten = 1.0;
        if l.radius > 0.0 {
            let d2 = to_l.dot(to_l);
            let r = l.radius;
            let w = (1.0 - (d2 * d2) / (r * r * r * r)).clamp(0.0, 1.0);
            atten = (w * w) / (d2 + 1.0);
        }
        if l.kind == LightKind::Spot {
            let cd = l.direction.dot(-dir);
            let (lo, hi) = (l.cone_outer.cos(), l.cone_inner.cos());
            let t = ((cd - lo) / (hi - lo)).clamp(0.0, 1.0);
            atten *= t * t * (3.0 - 2.0 * t);
        }
        (dir, atten)
    }

    /// The NEW loop, mirrored: `seed` is `ambient` for `mesh.wgsl`/`skinned.wgsl` and
    /// `Vec3::ZERO` for `mesh_textured.wgsl`'s zero-seeded diffuse accumulator.
    fn loop_diffuse(rig: &LightRig, seed: Vec3, n: Vec3, wp: Vec3) -> Vec3 {
        let mut acc = seed;
        for l in rig.lights.iter().take(rig.count as usize) {
            let (dir, atten) = light_sample(l, wp);
            let radiance = l.color * l.intensity;
            let ndl = n.dot(dir).max(0.0);
            acc += radiance * (ndl * atten);
        }
        acc
    }

    /// TODAY'S closed form, mirrored **exactly as each shader wrote it** — including the
    /// left-associative `+` chain, which is the only thing that can differ, f32 addition
    /// not being associative. `seed` is `Some(ambient)` for `mesh.wgsl`'s
    /// `scene.ambient.rgb + sun + moon + point`, and `None` for `mesh_textured.wgsl`'s
    /// bare `(sun_d + moon_d + point_d)` with the ambient added outside.
    fn closed_diffuse(rig: &LightRig, seed: Option<Vec3>, n: Vec3, wp: Vec3) -> Vec3 {
        let (sun, moon, point) = (&rig.lights[0], &rig.lights[1], &rig.lights[2]);
        let sun_d = sun.color * n.dot(sun.direction).max(0.0);
        let moon_d = moon.color * n.dot(moon.direction).max(0.0);
        let to_point = point.position - wp;
        let point_dir = to_point / to_point.length().max(1e-4);
        let point_d = point.color * n.dot(point_dir).max(0.0);
        match seed {
            Some(ambient) => ambient + sun_d + moon_d + point_d,
            None => sun_d + moon_d + point_d,
        }
    }

    /// **The NUMERIC half of S4a's identity gate — it proves the ARITHMETIC, never the
    /// shipped text.** The count-bounded loop must produce, bit for bit, what the
    /// hand-written `sun + moon + point` did, for the default rig AND for every rig
    /// shipped content actually compiles to — in BOTH accumulation orders
    /// (`mesh`/`skinned` seed the accumulator with ambient; `mesh_textured` seeds it
    /// with zero and adds ambient outside). Bit equality, not an epsilon: "zero pixel
    /// change" is the claim.
    ///
    /// BOTH sides below are Rust re-derivations, so this gate alone cannot see the WGSL
    /// drifting away from them. The SOURCE half lives in the crate that owns the
    /// shaders — `flicker-render`'s `the_lit_shaders_ship_the_light_loop_the_mirrors_assert`
    /// — which `include_str!`s the three lit shaders and asserts the seeds, the loop
    /// bound and the term spelling this mirror assumes. Neither half is load-bearing
    /// without the other; together they cover the channel.
    ///
    /// Scope: the presets written in the LEGACY form. The identity being proved is that
    /// the loop reproduces the closed form for the rigs that predate it, and a preset in
    /// the general form (`hearth`) has no closed form to be identical to — its falloff
    /// and its intensity are exactly the terms the closed form never had. The
    /// no-falloff/no-intensity path stays covered here by every legacy preset.
    #[test]
    fn the_light_loop_is_bit_identical_to_the_closed_form() {
        let styles = shipped_styles();
        let mut rigs = vec![("default".to_string(), LightRig::default())];
        for (name, _) in styles["stages"]["lighting"]
            .as_object()
            .expect("the preset table")
            .iter()
            .filter(|(n, v)| !n.starts_with('_') && v.get("lights").is_none())
        {
            let mut problems = Vec::new();
            rigs.push((
                name.clone(),
                compile_preset(&styles, name, &mut problems).expect("preset exists"),
            ));
        }
        assert!(rigs.len() >= 4, "default + the shipped presets");

        // A grid of surface normals and world positions — including the degenerate
        // "fragment sitting exactly on the point light" case the 1e-4 clamp exists for.
        let axes = [-1.0f32, -0.37, 0.0, 0.61, 1.0];
        let mut normals = Vec::new();
        for x in axes {
            for y in axes {
                for z in axes {
                    let n = Vec3::new(x, y, z);
                    normals.push(n.normalize_or_zero());
                }
            }
        }
        let positions = [
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-260.0, 44.0, 913.0),
            Vec3::new(0.004, -0.004, 0.004),
            Vec3::splat(1.0e5),
        ];

        let mut checked = 0usize;
        for (name, rig) in &rigs {
            for &wp in &positions {
                for &n in &normals {
                    for (order, seed) in [
                        ("mesh (ambient-seeded)", Some(rig.ambient)),
                        ("mesh_textured (zero-seeded)", None),
                    ] {
                        let a = loop_diffuse(rig, seed.unwrap_or(Vec3::ZERO), n, wp);
                        let b = closed_diffuse(rig, seed, n, wp);
                        for (i, (x, y)) in [(a.x, b.x), (a.y, b.y), (a.z, b.z)].iter().enumerate() {
                            assert_eq!(
                                x.to_bits(),
                                y.to_bits(),
                                "{name} / {order}: channel {i} differs at n={n:?} wp={wp:?} \
                                 ({x} vs {y})"
                            );
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 1000, "the grid actually ran ({checked} samples)");
    }

    /// **GATE — no shipped stage pairs a `skinned` layer with a non-directional light.**
    /// The skinned shader used to ignore the point light entirely; on the shared loop it
    /// gains point/spot reach, which is only invisible while no skinned stage carries
    /// one. A stage that does is a real visual change and must be a deliberate one.
    #[test]
    fn no_shipped_stage_pairs_a_skinned_layer_with_a_non_directional_light() {
        let mut checked = 0;
        let mut check = |where_: &str, styles: &Json, source: &str| {
            let Some((def, _)) = compile_stage(styles, source) else {
                return;
            };
            if !def.layers.contains(&StageLayer::Skinned) {
                return;
            }
            checked += 1;
            for (i, l) in def.lighting.lights[..def.lighting.count as usize]
                .iter()
                .enumerate()
            {
                assert!(
                    l.kind == LightKind::Dir || l.color == Vec3::ZERO,
                    "{where_} stages.{source} draws a `skinned` layer under a \
                     {:?} light (slot {i}) — the skinned pass now takes point/spot \
                     reach it used to drop",
                    l.kind
                );
            }
        };
        let shared = shipped_styles();
        for source in shared["stages"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| is_source_key(k))
        {
            check("ui_stages.json", &shared, source);
        }
        for (id, def, styles) in shipped_scenes() {
            for source in def.stages().into_iter().flat_map(|s| s.keys()) {
                check(&format!("{id}.scene.json"), &styles, source);
            }
        }
        assert!(checked > 0, "some shipped stage draws a skinned layer");
    }
    /// **The general `lights` form** — the vocabulary S4b's hearth is authored in.
    /// Every rule of §3 in one preset table: the array REPLACES the legacy trio (and
    /// says so), slots 0/1 are the SKY SLOTS and a fixed light parked there is a
    /// problem, each kind requires the fields it reads AND reads only the keys its kind
    /// has, an unknown kind / key / non-finite number is reported and costs only that
    /// light, a seed that is not a whole number is reported rather than rounded, a bad
    /// driver costs only the driver, an EMPTY roster is a problem, and a roster past
    /// `MAX_LIGHTS` is truncated loudly.
    #[test]
    fn the_general_lights_form_compiles_and_every_unknown_is_a_problem() {
        let styles = serde_json::json!({ "stages": { "lighting": {
            // The SHIPPED shape: the two sky slots first, then the fixed lights.
            "hearth": { "ambient": [0.1, 0.1, 0.12], "lights": [
                { "kind": "dir", "dir": [0.4, 0.8, 0.5], "color": [0.2, 0.2, 0.25] },
                { "kind": "dir", "dir": [-0.3, 0.7, 0.4], "color": [0.0, 0.0, 0.0] },
                { "kind": "point", "pos": [12.0, 1.4, -8.0], "color": [1.0, 0.55, 0.18],
                  "intensity": 8.0, "radius": 16.0,
                  "driver": { "kind": "flicker", "speed": 7.0, "depth": 0.35, "seed": 1 } },
                { "kind": "spot", "pos": [0.0, 4.0, 0.0], "dir": [0.0, -1.0, 0.0],
                  "color": [1.0, 1.0, 1.0], "cone": [12.0, 30.0], "radius": 20.0 }
            ]},
            "both": { "sun": [1.0, 1.0, 1.0], "lights": [
                { "kind": "dir", "dir": [0.0, 1.0, 0.0], "color": [0.5, 0.5, 0.5] }
            ]},
            // A fixed light in slot 0 and the fire in slot 1: the silent-clobber shape.
            "misplaced": { "lights": [
                { "kind": "point", "pos": [0.0, 0.0, 0.0], "color": [1.0, 0.5, 0.2] },
                { "kind": "spot", "pos": [0.0, 4.0, 0.0], "dir": [0.0, -1.0, 0.0],
                  "cone": [12.0, 30.0] },
                { "kind": "dir", "dir": [0.0, 1.0, 0.0], "color": [1.0, 1.0, 1.0] }
            ]},
            "dark": { "lights": [] },
            "sloppy": { "lights": [
                { "kind": "lantern", "pos": [0.0, 0.0, 0.0] },
                { "kind": "point" },
                { "kind": "spot", "pos": [0.0, 1.0, 0.0] },
                { "kind": "dir", "dir": [0.0, 1.0, 0.0], "glow": 3.0,
                  "intensity": "bright",
                  "driver": { "kind": "strobe", "speed": 2.0 } },
                // Right key, wrong kind: all three are packed and then discarded.
                { "kind": "dir", "dir": [0.0, 1.0, 0.0], "pos": [1.0, 2.0, 3.0],
                  "radius": 4.0, "cone": [10.0, 20.0],
                  "driver": { "kind": "pulse", "speed": 1.0, "seed": -3 } },
                { "kind": "point", "pos": [0.0, 0.0, 0.0], "cone": [10.0, 20.0],
                  "driver": { "kind": "flicker", "speed": 2.0, "seed": 1.5 } }
            ]},
            "crowded": { "lights": (0..MAX_LIGHTS + 2).map(|i| serde_json::json!(
                { "kind": "point", "pos": [i as f64, 0.0, 0.0], "color": [1.0, 1.0, 1.0] }
            )).collect::<Vec<_>>() }
        }}});

        let mut problems = Vec::new();
        let hearth = compile_preset(&styles, "hearth", &mut problems).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            hearth.count, 4,
            "the array IS the roster — no legacy trio behind it"
        );
        assert_eq!(hearth.lights[2].kind, LightKind::Point);
        assert_eq!(hearth.lights[2].intensity, 8.0);
        assert_eq!(hearth.lights[2].radius, 16.0);
        assert_eq!(
            hearth.lights[2].driver,
            Some(Driver {
                kind: DriverKind::Flicker,
                speed: 7.0,
                depth: 0.35,
                seed: 1
            })
        );
        assert_eq!(hearth.lights[0].kind, LightKind::Dir);
        assert!(
            (hearth.lights[0].direction.length() - 1.0).abs() < 1e-6,
            "an authored direction is normalized"
        );
        assert_eq!(hearth.lights[3].kind, LightKind::Spot);
        assert!((hearth.lights[3].cone_inner - 12.0_f32.to_radians()).abs() < 1e-6);
        assert!((hearth.lights[3].cone_outer - 30.0_f32.to_radians()).abs() < 1e-6);
        // The sky reads the SLOTS, not the first directional light it can find.
        assert_eq!(hearth.sky_sun(), hearth.lights[0], "slot 0 is the sun");
        assert_eq!(hearth.sky_moon(), hearth.lights[1], "slot 1 is the moon");

        // A fixed light in a sky slot is LOUD — the cycle would eat it every frame,
        // and the sky reads black rather than the light standing there.
        let mut problems = Vec::new();
        let misplaced = compile_preset(&styles, "misplaced", &mut problems).unwrap();
        let text = problems.join("\n");
        for expected in [
            "lights slot 0 holds a `point` light",
            "lights slot 1 holds a `spot` light",
            "put fixed lights at slot 2+",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
        }
        assert_eq!(
            misplaced.count, 3,
            "the lights still compile — the problem is the SLOT, not the light"
        );
        assert_eq!(
            misplaced.sky_sun().color,
            Vec3::ZERO,
            "a non-dir slot 0 darkens the sky rather than being read as a sun"
        );
        assert_eq!(
            misplaced.sky_moon().color,
            Vec3::ZERO,
            "…and so does a non-dir slot 1: the dir light in slot 2 is NOT promoted"
        );

        // An empty roster lights nothing, and says so.
        let mut problems = Vec::new();
        let dark = compile_preset(&styles, "dark", &mut problems).unwrap();
        assert_eq!(dark.count, 0, "an empty array is an empty roster");
        assert!(
            problems
                .iter()
                .any(|p| p.contains("an empty roster lights nothing")),
            "{problems:?}"
        );

        // Both forms in one preset: reported, and the array wins.
        let mut problems = Vec::new();
        let both = compile_preset(&styles, "both", &mut problems).unwrap();
        assert_eq!(both.count, 1, "the array wins");
        assert!(
            problems
                .iter()
                .any(|p| p.contains("authors both the legacy")),
            "{problems:?}"
        );

        // Every malformed light costs itself and nothing else; a bad driver costs only
        // the driver.
        let mut problems = Vec::new();
        let sloppy = compile_preset(&styles, "sloppy", &mut problems).unwrap();
        let text = problems.join("\n");
        for expected in [
            "names `lantern`, which is not a light kind",
            "is a `point` light with no pos",
            "is a `spot` light with no dir / cone",
            "is not a light key (`glow`)",
            "intensity must be a finite number",
            "names `strobe`, which is not a driver kind",
            // Spelled right, read by nothing — the accepted-but-discarded hole.
            "pos is not a key a `dir` light reads",
            "radius is not a key a `dir` light reads",
            "cone is not a key a `dir` light reads",
            "cone is not a key a `point` light reads",
            // The one integer in the vocabulary is read as one, never rounded.
            "seed must be a whole number in 0..=4294967295",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
        }
        assert_eq!(
            text.matches("must be a whole number").count(),
            2,
            "both the negative seed AND the fractional one are reported:\n{text}"
        );
        assert_eq!(
            sloppy.count, 3,
            "the three lights that state the fields their kind reads survived"
        );
        assert_eq!(sloppy.lights[0].kind, LightKind::Dir);
        assert!(
            sloppy.lights[0].driver.is_none(),
            "the light shines undriven"
        );
        assert_eq!(
            sloppy.lights[1].driver.map(|d| d.seed),
            Some(0),
            "a rejected seed keeps the default rather than clamping the authored one"
        );

        // Past the cap: truncated, and loud about it — and loud, separately, about the
        // two point lights that landed in the sky slots.
        let mut problems = Vec::new();
        let crowded = compile_preset(&styles, "crowded", &mut problems).unwrap();
        assert_eq!(crowded.count as usize, MAX_LIGHTS);
        assert!(
            problems.iter().any(|p| p.contains("at most")),
            "{problems:?}"
        );
        assert_eq!(
            problems
                .iter()
                .filter(|p| p.contains("are the sky slots"))
                .count(),
            2,
            "both sky slots report the point light standing in them: {problems:?}"
        );
    }
}
