//! The Air-Traffic-Control **simulation** — the whole game as plain state plus one
//! [`Game::tick`] per radar sweep. No engine types, no rendering, no UI: the scene
//! above reads this and draws it, and every rule here is exercised without a window.
//!
//! # The world
//!
//! An 11 × 12 grid of sectors. Six **flight corridors** pierce its boundary (ZAN /
//! CAN / ZMP / ZDV / ZOA / APAC) and two **airports** sit inside it, each with a
//! single runway and therefore two possible landing directions — the prevailing
//! wind picks one per game ([`Game::landing_heading`]) and both **outer markers**
//! stay drawn, exactly as the paper spec describes. Two **holding fixes** (A and B)
//! park traffic that cannot be sequenced yet.
//!
//! # The clock
//!
//! One tick is one radar sweep. A sweep is 30 simulated seconds, which fixes every
//! other number honestly rather than by taste:
//!
//! * a **prop** (150 mph) covers half a sector, a **jet** (300 mph) a whole one —
//!   which is precisely the paper spec's "aircraft may advance in ½ grid increments";
//! * climb / descent is [`CLIMB_FT`] per sweep;
//! * 15 minutes of fuel is [`FUEL_TICKS`] sweeps, and the pilot calls low at 10.
//!
//! # The one interpretation
//!
//! The paper spec says a close encounter is two aircraft "within 1,000 ft of one
//! another horizontally **or** vertically". Read as OR, every pair sharing an
//! altitude anywhere on the scope would end the game the instant it started, so the
//! only playable reading — and the standard separation rule — is AND: closer than
//! [`SEPARATION_GRID`] horizontally **and** [`SEPARATION_FT`] vertically.

// ── The board ────────────────────────────────────────────────────────────────

/// Sectors across the controller's area.
pub const COLS: f32 = 11.0;
/// Sectors down the controller's area.
pub const ROWS: f32 = 12.0;

/// Simulated seconds in one radar sweep — the unit every other constant is in.
pub const SWEEP_SIM_SECONDS: f32 = 30.0;
/// Real seconds a sweep takes to play out. The paper spec's 20-30 s sweep makes a
/// one-hour session; this is the demo's tempo, and it is the ONE speed (no rate
/// control — a toy on strict rails).
pub const SWEEP_REAL_SECONDS: f32 = 3.0;

/// Degrees an aircraft turns per sweep. A turn always ENDS on the commanded
/// heading, so a heading readout is only ever a number the controller asked for.
pub const TURN_RATE: i32 = 45;
/// Feet climbed or lost per sweep.
pub const CLIMB_FT: i32 = 1000;
/// Altitude an aircraft enters the area at, and the only altitude it may leave at.
pub const ENTRY_ALT: i32 = 10_000;
/// Pattern altitude: a departure leaves the field at it, an arrival must be at it
/// by the outer marker.
pub const FIELD_ALT: i32 = 1_000;
/// The ceiling a controller may assign, in feet.
pub const MAX_ALT: i32 = 10_000;

/// Sweeps of fuel on arrival / on reporting ready — 15 minutes.
pub const FUEL_TICKS: u32 = 30;
/// The pilot calls low on fuel with this many sweeps left — after 10 minutes.
pub const LOW_FUEL_TICKS: u32 = 10;

/// Horizontal half of the separation minimum, in sectors.
pub const SEPARATION_GRID: f32 = 1.0;
/// Vertical half of the separation minimum, in feet.
pub const SEPARATION_FT: i32 = 1000;

/// Sectors from the field to its outer markers, along the runway axis.
pub const MARKER_DIST: f32 = 1.6;
/// How near an aircraft's track must pass an outer marker to shoot the approach.
pub const CAPTURE_GRID: f32 = 0.6;
/// Degrees of runway alignment allowed at the marker.
pub const HEADING_TOL: i32 = 10;
/// How near a departing aircraft must leave its assigned corridor, in sectors.
pub const EXIT_TOL: f32 = 1.0;
/// How near a holding aircraft must be to its fix before it starts to orbit.
pub const HOLD_CAPTURE: f32 = 0.7;

/// The full traffic roster — one aircraft per designation, A through Z.
pub const ROSTER: usize = 26;
/// Sweeps between arrivals, inclusive range (1½ to 3 minutes).
pub const SPAWN_GAP: (u32, u32) = (3, 6);

/// One of the six flight corridors on the boundary of the controller's area.
pub struct Corridor {
    /// The centre's call sign, as the pilot says it.
    pub id: &'static str,
    pub x: f32,
    pub y: f32,
    /// The heading an aircraft must be flying to leave through this corridor.
    pub exit_heading: i32,
}

impl Corridor {
    /// The heading an aircraft is flying when it enters through this corridor —
    /// the reciprocal of the way out.
    pub fn entry_heading(&self) -> i32 {
        wrap(self.exit_heading + 180)
    }
}

/// The six corridors, laid out as the paper spec's scope shows them.
pub const CORRIDORS: [Corridor; 6] = [
    Corridor { id: "ZAN", x: 2.0, y: 0.0, exit_heading: 360 },
    Corridor { id: "CAN", x: 6.0, y: 0.0, exit_heading: 360 },
    Corridor { id: "ZMP", x: COLS, y: 6.0, exit_heading: 90 },
    Corridor { id: "ZDV", x: COLS, y: 9.0, exit_heading: 90 },
    Corridor { id: "ZOA", x: 5.0, y: ROWS, exit_heading: 180 },
    Corridor { id: "APAC", x: 0.0, y: 6.0, exit_heading: 270 },
];

/// One airport: a position and the two directions its single runway can be used
/// in. Which one is live is a per-game roll — the prevailing wind.
pub struct Airport {
    pub id: &'static str,
    pub x: f32,
    pub y: f32,
    /// The two runway headings, reciprocals of one another.
    pub axis: [i32; 2],
}

/// The two fields. Boeing is the paper spec's "single line airport", whose 130/310
/// axis is the worked example in the rules.
pub const AIRPORTS: [Airport; 2] = [
    Airport { id: "BFI", x: 3.4, y: 3.2, axis: [130, 310] },
    Airport { id: "SEA", x: 6.0, y: 6.5, axis: [170, 350] },
];

/// A holding fix — the racetrack an aircraft parks on until it can be sequenced.
pub struct HoldFix {
    pub id: &'static str,
    pub x: f32,
    pub y: f32,
}

/// Holding points A and B. Both sit far enough inside the boundary that even a jet
/// orbiting one at the widest it can hold stays on the scope.
pub const HOLDS: [HoldFix; 2] = [
    HoldFix { id: "A", x: 1.8, y: 8.4 },
    HoldFix { id: "B", x: 9.2, y: 2.4 },
];

// ── Headings ─────────────────────────────────────────────────────────────────

/// Fold any degree value onto the controller's `1..=360` naming, where 360 is
/// north (there is no "heading zero" on a strip).
pub fn wrap(deg: i32) -> i32 {
    let m = deg.rem_euclid(360);
    if m == 0 {
        360
    } else {
        m
    }
}

/// A signed-free turn arc: how many degrees clockwise from `from` to `to`.
fn arc_right(from: i32, to: i32) -> i32 {
    (to - from).rem_euclid(360)
}

/// The unit track vector for `heading`, in sector units with y running DOWN the
/// scope (north is −y).
pub fn track(heading: i32) -> (f32, f32) {
    let r = (heading as f32).to_radians();
    (r.sin(), -r.cos())
}

/// The heading that points from `(x, y)` at `(tx, ty)`, snapped to the 10° the
/// controller can actually say.
pub fn bearing(x: f32, y: f32, tx: f32, ty: f32) -> i32 {
    let deg = (tx - x).atan2(y - ty).to_degrees();
    wrap(((deg / 10.0).round() as i32) * 10)
}

/// One step of a turn: `TURN_RATE` toward `to` the commanded way round, landing
/// exactly on `to` once it is within reach.
fn turn_step(from: i32, to: i32, right: bool) -> i32 {
    let arc = if right { arc_right(from, to) } else { arc_right(to, from) };
    if arc <= TURN_RATE {
        wrap(to)
    } else if right {
        wrap(from + TURN_RATE)
    } else {
        wrap(from - TURN_RATE)
    }
}

/// Distance from `p` to the segment `a`→`b`. A jet crosses a whole sector per
/// sweep, so an outer marker is tested against the TRACK, never against the two
/// end points it may have straddled.
fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + dx * t, ay + dy * t);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

// ── Aircraft ─────────────────────────────────────────────────────────────────

/// What is flying: the two airframes, which is the whole of "performance" here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 150 mph — half a sector per sweep.
    Prop,
    /// 300 mph — a whole sector per sweep.
    Jet,
}

impl Kind {
    /// Sectors covered in one sweep.
    pub fn speed(self) -> f32 {
        match self {
            Kind::Prop => 0.5,
            Kind::Jet => 1.0,
        }
    }

    /// The one-glyph strip mark — bold `J` for a jet, plain `P` for a prop, so the
    /// controller can tell at a glance what is about to overtake what.
    pub fn mark(self) -> &'static str {
        match self {
            Kind::Prop => "P",
            Kind::Jet => "J",
        }
    }
}

/// Where an aircraft has been told to end up. The paper spec's three scenarios
/// collapse to two destinations: a pass-through and a departure both LEAVE by a
/// corridor, and only the entry point differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Leave the area through this corridor, at [`ENTRY_ALT`].
    Depart(usize),
    /// Land at this airport.
    Land(usize),
}

/// What the aircraft is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Sitting at its field, waiting on a `CT`. Burns fuel; does not move; cannot
    /// lose separation.
    Ready,
    /// Flying the assigned heading.
    Airborne,
    /// Working toward, or orbiting, a holding fix.
    Holding(usize),
}

/// One aircraft on the scope.
#[derive(Clone, Debug)]
pub struct Aircraft {
    /// The designation the pilot uses — A through Z, unique for the session.
    pub id: char,
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    /// Current heading, `1..=360`.
    pub heading: i32,
    /// The heading it is turning onto (equal to `heading` when steady).
    pub target_heading: i32,
    /// Which way round the controller sent it.
    pub turn_right: bool,
    pub alt: i32,
    /// The altitude it is climbing or descending to.
    pub target_alt: i32,
    pub plan: Plan,
    pub phase: Phase,
    /// Sweeps of fuel left.
    pub fuel: u32,
    /// The corridor it arrived through, or `None` for a departure off a field.
    pub entry: Option<usize>,
    /// The field it is departing from, or `None` for an arrival.
    pub base: Option<usize>,
}

impl Aircraft {
    /// The destination as the strip prints it — a corridor call sign or a field.
    pub fn destination(&self) -> &'static str {
        match self.plan {
            Plan::Depart(c) => CORRIDORS[c].id,
            Plan::Land(a) => AIRPORTS[a].id,
        }
    }

    /// Where it came from, as the strip prints it.
    pub fn origin(&self) -> &'static str {
        match (self.entry, self.base) {
            (Some(c), _) => CORRIDORS[c].id,
            (_, Some(a)) => AIRPORTS[a].id,
            _ => "--",
        }
    }

    /// Is the fuel state one the pilot has already complained about?
    pub fn low_fuel(&self) -> bool {
        self.fuel <= LOW_FUEL_TICKS
    }

    /// Altitude in the thousands the strips and the data tags read in.
    pub fn flight_level(&self) -> i32 {
        self.alt / 1000
    }
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// The controller's whole vocabulary — the paper spec's command list, one variant
/// each. Every one of them renders to its canonical code with [`Cmd::code`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// `TL##` — turn left onto a heading.
    TurnLeft(i32),
    /// `TR##` — turn right onto a heading.
    TurnRight(i32),
    /// `DH##` — descend and hold thousands of feet.
    DescendHold(i32),
    /// `AH##` — ascend and hold thousands of feet.
    AscendHold(i32),
    /// `HA` / `HB` — hold at a fix.
    HoldAt(usize),
    /// `CT` — cleared for take-off.
    ClearedTakeoff,
}

impl Cmd {
    /// The canonical code, without the aircraft prefix (`TL09`, `DH03`, `HA`, `CT`).
    pub fn code(self) -> String {
        match self {
            Cmd::TurnLeft(h) => format!("TL{:02}", h / 10),
            Cmd::TurnRight(h) => format!("TR{:02}", h / 10),
            Cmd::DescendHold(ft) => format!("DH{:02}", ft / 1000),
            Cmd::AscendHold(ft) => format!("AH{:02}", ft / 1000),
            Cmd::HoldAt(i) => format!("H{}", HOLDS[i].id),
            Cmd::ClearedTakeoff => "CT".to_string(),
        }
    }
}

/// Why a transmission was not accepted. Each maps to one readback line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// No aircraft of that designation is on the scope.
    NoSuchFlight,
    /// It is still on the ground: it can only take a `CT`.
    OnTheGround,
    /// It is already flying: `CT` means nothing to it.
    Airborne,
    /// `DH` above, or `AH` below, where it already is.
    WrongDirection,
    /// A heading or an altitude outside what an aircraft can be given.
    OutOfRange,
}

// ── Events ───────────────────────────────────────────────────────────────────

/// A pilot communication — what the strip bay announces, in the order it happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Checked in on entering the area.
    Entered(char),
    /// Reported ready for take-off at its field.
    Ready(char),
    /// Handed off to approach control and down.
    Landed(char),
    /// Left cleanly through its assigned corridor.
    Departed(char),
    /// Ten minutes gone: the pilot is calling it in.
    LowFuel(char),
    /// Crossed the marker unaligned or high — going around.
    WentAround(char),
}

impl Event {
    /// The aircraft the call is about.
    pub fn flight(self) -> char {
        match self {
            Event::Entered(c)
            | Event::Ready(c)
            | Event::Landed(c)
            | Event::Departed(c)
            | Event::LowFuel(c)
            | Event::WentAround(c) => c,
        }
    }
}

/// How the session ended. The paper spec's four losses, plus the win.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Two aircraft inside the separation minimum.
    Conflict(char, char),
    /// Someone ran the tanks dry.
    OutOfFuel(char),
    /// Left the area anywhere but the corridor it was cleared to.
    WrongExit(char),
    /// Left the assigned corridor at anything but [`ENTRY_ALT`].
    WrongAltitude(char),
    /// Every one of the 26 handled, and the scope is clear.
    Cleared,
}

impl Verdict {
    /// Did the session end well?
    pub fn is_win(self) -> bool {
        matches!(self, Verdict::Cleared)
    }
}

// ── The game ─────────────────────────────────────────────────────────────────

/// The whole session: the traffic, the score, the runway assignment, and the
/// roll that decides what shows up next.
pub struct Game {
    /// Everything on the scope, in the order it arrived.
    pub aircraft: Vec<Aircraft>,
    pub landed: u32,
    pub departed: u32,
    /// Set once, and then the session is over.
    pub verdict: Option<Verdict>,
    /// Which end of each field's runway the wind picked, as an index into
    /// [`Airport::axis`].
    pub landing_dir: [usize; 2],
    /// Sweeps flown.
    pub sweep: u64,
    /// How many aircraft this session deals in total — [`ROSTER`] for a game.
    roster: usize,
    /// How many of the roster have checked in.
    spawned: usize,
    /// Sweeps until the next arrival.
    next_in: u32,
    rng: fastrand::Rng,
}

impl Game {
    /// A fresh session of the full [`ROSTER`]. `seed` fixes the runway assignment,
    /// the airframes and every scenario roll, so a test drives an exact scenario
    /// and a player gets a different hour each time.
    pub fn new(seed: u64) -> Self {
        Self::with_roster(seed, ROSTER)
    }

    /// A session of exactly `roster` aircraft. The full 26 is the game; a shorter
    /// deal is how one rule is watched in isolation, with no other traffic arriving
    /// to muddy it.
    pub fn with_roster(seed: u64, roster: usize) -> Self {
        let rng = fastrand::Rng::with_seed(seed);
        let mut me = Self {
            aircraft: Vec::new(),
            landed: 0,
            departed: 0,
            verdict: None,
            landing_dir: [0, 0],
            sweep: 0,
            roster: roster.min(ROSTER),
            spawned: 0,
            next_in: 0,
            rng,
        };
        me.landing_dir = [me.rng.usize(0..2), me.rng.usize(0..2)];
        // The first aircraft is already on the scope when the controller sits down
        // — an empty board teaches nothing.
        me.spawn();
        me
    }

    /// The runway heading in use at `airport` this session — the number printed
    /// beside the field.
    pub fn landing_heading(&self, airport: usize) -> i32 {
        AIRPORTS[airport].axis[self.landing_dir[airport]]
    }

    /// The LIVE outer marker for `airport` — the one upwind of the field, which is
    /// the one an arrival crosses. Its opposite number is drawn too and is inert.
    pub fn outer_marker(&self, airport: usize) -> (f32, f32) {
        let (dx, dy) = track(self.landing_heading(airport));
        let a = &AIRPORTS[airport];
        (a.x - dx * MARKER_DIST, a.y - dy * MARKER_DIST)
    }

    /// The inert marker on the far side of the field.
    pub fn far_marker(&self, airport: usize) -> (f32, f32) {
        let (dx, dy) = track(self.landing_heading(airport));
        let a = &AIRPORTS[airport];
        (a.x + dx * MARKER_DIST, a.y + dy * MARKER_DIST)
    }

    /// How many of the roster have not yet checked in.
    pub fn pending(&self) -> usize {
        ROSTER - self.spawned
    }

    /// Find an aircraft by designation.
    pub fn find(&self, id: char) -> Option<&Aircraft> {
        self.aircraft.iter().find(|a| a.id == id)
    }

    /// Transmit to `id`. On acceptance the aircraft acts immediately and the
    /// canonical transmission (`STL09`) comes back for the log; on refusal nothing
    /// changes and the reason comes back instead.
    pub fn command(&mut self, id: char, cmd: Cmd) -> Result<String, Reject> {
        if self.verdict.is_some() {
            return Err(Reject::NoSuchFlight);
        }
        let Some(a) = self.aircraft.iter_mut().find(|a| a.id == id) else {
            return Err(Reject::NoSuchFlight);
        };
        let grounded = a.phase == Phase::Ready;
        match cmd {
            Cmd::ClearedTakeoff => {
                if !grounded {
                    return Err(Reject::Airborne);
                }
                // Off the field into the pattern, on the runway heading — the wind
                // that decides where you land decides where you leave from.
                let base = a.base.expect("a Ready aircraft sits at a field");
                let hdg = AIRPORTS[base].axis[self.landing_dir[base]];
                a.phase = Phase::Airborne;
                a.heading = hdg;
                a.target_heading = hdg;
                a.alt = FIELD_ALT;
                a.target_alt = FIELD_ALT;
            }
            _ if grounded => return Err(Reject::OnTheGround),
            Cmd::TurnLeft(h) | Cmd::TurnRight(h) => {
                if h % 10 != 0 || !(1..=360).contains(&h) {
                    return Err(Reject::OutOfRange);
                }
                a.target_heading = wrap(h);
                a.turn_right = matches!(cmd, Cmd::TurnRight(_));
                a.phase = Phase::Airborne; // a vector cancels a hold
            }
            Cmd::DescendHold(ft) => {
                if !(0..=MAX_ALT).contains(&ft) || ft % 1000 != 0 {
                    return Err(Reject::OutOfRange);
                }
                if ft >= a.alt {
                    return Err(Reject::WrongDirection);
                }
                a.target_alt = ft;
            }
            Cmd::AscendHold(ft) => {
                if !(0..=MAX_ALT).contains(&ft) || ft % 1000 != 0 {
                    return Err(Reject::OutOfRange);
                }
                if ft <= a.alt {
                    return Err(Reject::WrongDirection);
                }
                a.target_alt = ft;
            }
            Cmd::HoldAt(fix) => {
                if fix >= HOLDS.len() {
                    return Err(Reject::OutOfRange);
                }
                a.phase = Phase::Holding(fix);
            }
        }
        Ok(format!("{id}{}", cmd.code()))
    }

    /// One radar sweep. Returns the pilot calls it produced, oldest first; a
    /// [`Verdict`] may be set, after which further ticks do nothing.
    pub fn tick(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if self.verdict.is_some() {
            return events;
        }
        self.sweep += 1;

        // The fuel clock runs on the ground and in the air alike.
        for a in &mut self.aircraft {
            a.fuel = a.fuel.saturating_sub(1);
            if a.fuel == LOW_FUEL_TICKS {
                events.push(Event::LowFuel(a.id));
            }
        }
        if let Some(dry) = self.aircraft.iter().find(|a| a.fuel == 0) {
            self.verdict = Some(Verdict::OutOfFuel(dry.id));
            return events;
        }

        // The approach geometry, read once — it is fixed for the session and the
        // flight loop below holds the aircraft mutably.
        let markers: Vec<(f32, f32)> = (0..AIRPORTS.len()).map(|i| self.outer_marker(i)).collect();
        let land_hdg: Vec<i32> = (0..AIRPORTS.len()).map(|i| self.landing_heading(i)).collect();

        let mut done: Vec<usize> = Vec::new();
        let mut verdict = None;
        for i in 0..self.aircraft.len() {
            let a = &mut self.aircraft[i];
            if a.phase == Phase::Ready {
                continue;
            }
            // A hold steers itself: run for the fix, then fly TANGENT to it. The
            // tangent is taken from the CURRENT bearing every sweep, so the orbit
            // is self-correcting — an aircraft that drifts wide turns back rather
            // than free-wheeling off the scope. Any vector the controller gives
            // cancels the hold (see `command`).
            if let Phase::Holding(fix) = a.phase {
                let f = &HOLDS[fix];
                let to_fix = bearing(a.x, a.y, f.x, f.y);
                let d = (a.x - f.x).hypot(a.y - f.y);
                a.target_heading = if d <= HOLD_CAPTURE { wrap(to_fix + 90) } else { to_fix };
                a.turn_right = true;
            }

            a.heading = turn_step(a.heading, a.target_heading, a.turn_right);
            a.alt += (a.target_alt - a.alt).clamp(-CLIMB_FT, CLIMB_FT);

            let (px, py) = (a.x, a.y);
            let (dx, dy) = track(a.heading);
            let speed = a.kind.speed();
            a.x += dx * speed;
            a.y += dy * speed;

            // The approach: cross the live outer marker aligned with the runway and
            // at pattern altitude and approach control takes it; miss either and it
            // flies over the field and has to be brought round again.
            if let Plan::Land(ap) = a.plan {
                let (mx, my) = markers[ap];
                if seg_dist(mx, my, px, py, a.x, a.y) <= CAPTURE_GRID {
                    let off = arc_right(a.heading, land_hdg[ap]).min(arc_right(land_hdg[ap], a.heading));
                    if off <= HEADING_TOL && a.alt == FIELD_ALT {
                        events.push(Event::Landed(a.id));
                        done.push(i);
                        continue;
                    }
                    events.push(Event::WentAround(a.id));
                }
            }

            // The boundary: leaving is only ever right or fatal.
            if a.x < 0.0 || a.x > COLS || a.y < 0.0 || a.y > ROWS {
                match a.plan {
                    Plan::Depart(c)
                        if (a.x - CORRIDORS[c].x).hypot(a.y - CORRIDORS[c].y) <= EXIT_TOL =>
                    {
                        if a.alt == ENTRY_ALT {
                            events.push(Event::Departed(a.id));
                            done.push(i);
                        } else {
                            verdict = Some(Verdict::WrongAltitude(a.id));
                        }
                    }
                    _ => verdict = Some(Verdict::WrongExit(a.id)),
                }
                if verdict.is_some() {
                    break;
                }
            }
        }

        if let Some(v) = verdict {
            self.verdict = Some(v);
            return events;
        }

        // Retire the finished, highest index first so the rest keep their places.
        for i in done.into_iter().rev() {
            let a = self.aircraft.remove(i);
            match a.plan {
                Plan::Land(_) => self.landed += 1,
                Plan::Depart(_) => self.departed += 1,
            }
        }

        // Separation, over what is actually flying.
        if let Some((a, b)) = self.conflict() {
            self.verdict = Some(Verdict::Conflict(a, b));
            return events;
        }

        // New traffic.
        if self.spawned < self.roster {
            self.next_in = self.next_in.saturating_sub(1);
            if self.next_in == 0 {
                let id = self.spawn();
                events.push(match self.find(id).map(|a| a.phase) {
                    Some(Phase::Ready) => Event::Ready(id),
                    _ => Event::Entered(id),
                });
            }
        } else if self.aircraft.is_empty() {
            self.verdict = Some(Verdict::Cleared);
        }
        events
    }

    /// The first pair inside the separation minimum, if any. Aircraft still on the
    /// ground are not flying and cannot lose separation.
    fn conflict(&self) -> Option<(char, char)> {
        let air: Vec<&Aircraft> =
            self.aircraft.iter().filter(|a| a.phase != Phase::Ready).collect();
        for (i, a) in air.iter().enumerate() {
            for b in &air[i + 1..] {
                let near = (a.x - b.x).hypot(a.y - b.y) < SEPARATION_GRID;
                if near && (a.alt - b.alt).abs() < SEPARATION_FT {
                    return Some((a.id, b.id));
                }
            }
        }
        None
    }

    /// Roll the next aircraft onto the scope and re-arm the arrival clock.
    /// Returns its designation.
    fn spawn(&mut self) -> char {
        let id = (b'A' + self.spawned as u8) as char;
        self.spawned += 1;
        self.next_in = self.rng.u32(SPAWN_GAP.0..=SPAWN_GAP.1);
        let kind = if self.rng.bool() { Kind::Jet } else { Kind::Prop };

        // The paper spec's three scenarios, evenly rolled.
        let a = match self.rng.u32(0..3) {
            // Pass through: in one corridor, out another.
            0 => {
                let entry = self.rng.usize(0..CORRIDORS.len());
                let mut exit = self.rng.usize(0..CORRIDORS.len());
                if exit == entry {
                    exit = (exit + 1) % CORRIDORS.len();
                }
                let c = &CORRIDORS[entry];
                let h = c.entry_heading();
                Aircraft {
                    id,
                    kind,
                    x: c.x,
                    y: c.y,
                    heading: h,
                    target_heading: h,
                    turn_right: true,
                    alt: ENTRY_ALT,
                    target_alt: ENTRY_ALT,
                    plan: Plan::Depart(exit),
                    phase: Phase::Airborne,
                    fuel: FUEL_TICKS,
                    entry: Some(entry),
                    base: None,
                }
            }
            // Entry landing: in one corridor, down at a field.
            1 => {
                let entry = self.rng.usize(0..CORRIDORS.len());
                let field = self.rng.usize(0..AIRPORTS.len());
                let c = &CORRIDORS[entry];
                let h = c.entry_heading();
                Aircraft {
                    id,
                    kind,
                    x: c.x,
                    y: c.y,
                    heading: h,
                    target_heading: h,
                    turn_right: true,
                    alt: ENTRY_ALT,
                    target_alt: ENTRY_ALT,
                    plan: Plan::Land(field),
                    phase: Phase::Airborne,
                    fuel: FUEL_TICKS,
                    entry: Some(entry),
                    base: None,
                }
            }
            // Take-off departure: off a field, out a corridor.
            _ => {
                let field = self.rng.usize(0..AIRPORTS.len());
                let exit = self.rng.usize(0..CORRIDORS.len());
                let ap = &AIRPORTS[field];
                let h = ap.axis[self.landing_dir[field]];
                Aircraft {
                    id,
                    kind,
                    x: ap.x,
                    y: ap.y,
                    heading: h,
                    target_heading: h,
                    turn_right: true,
                    alt: 0,
                    target_alt: 0,
                    plan: Plan::Depart(exit),
                    phase: Phase::Ready,
                    fuel: FUEL_TICKS,
                    entry: None,
                    base: Some(field),
                }
            }
        };
        self.aircraft.push(a);
        id
    }
}
