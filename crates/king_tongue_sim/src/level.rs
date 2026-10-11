//! Level data: the schema, and parsing it from bytes.
//!
//! The Core Sim never touches the filesystem. The Shell reads bytes however its
//! platform requires and hands them here, which is what keeps ADR-0001's boundary
//! intact while still letting the deterministic parser live beside the types it
//! produces. See ADR-0010.

use glam::Vec2;

/// An axis-aligned box, used for both terrain and world bounds.
///
/// Stored as centre plus half-extents rather than min plus max: it makes
/// `contains` and overlap tests cheaper, and it matches how terrain is authored
/// (a tile at a position with a size).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Aabb {
    pub centre: Vec2,
    pub half_extents: Vec2,
}

impl Aabb {
    pub fn new(centre: Vec2, half_extents: Vec2) -> Self {
        Self {
            centre,
            half_extents,
        }
    }

    pub fn min(&self) -> Vec2 {
        self.centre - self.half_extents
    }

    pub fn max(&self) -> Vec2 {
        self.centre + self.half_extents
    }

    pub fn contains(&self, p: Vec2) -> bool {
        let d = (p - self.centre).abs();
        d.x <= self.half_extents.x && d.y <= self.half_extents.y
    }
}

/// Where an Enemy starts, and how hard it is to destroy.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EnemySpawn {
    pub pos: Vec2,
    /// Denominated in momentum units, per the Slam semantics resolution. An
    /// impact spends its normal momentum against this pool.
    pub toughness: f32,
    pub mass: f32,
}

/// One level.
///
/// Deliberately minimal: this is the Placeholder Geometry acceptance bar and
/// nothing more. No waves, no Enemy archetypes, no projectiles, no art. Those sit
/// in the map's fog and must not leak into this schema early, because every field
/// here becomes something Phase 4 has to implement.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Level {
    /// Hard global coordinates. A Thrown Enemy leaving this box despawns.
    ///
    /// This is an `Aabb` and not a single `Vec2`: a lone vector cannot express a
    /// rectangle without silently assuming an origin corner, and the Slam
    /// resolution requires a world-space kill bound precisely so that despawn is
    /// device-independent. Getting this wrong would reintroduce the determinism
    /// bug ADR-0002 exists to prevent.
    pub world_bounds: Aabb,

    /// Every one of these is valid anchor terrain. Per the Anchor validity
    /// resolution there are no ungrappable surfaces, so there is no separate
    /// anchor list: terrain *is* the anchor set.
    pub static_terrain: Vec<Aabb>,

    /// Where the hero starts. Absent from the original schema draft, and the
    /// scene cannot be built without it.
    pub player_spawn: Vec2,

    /// Enemy entry definitions.
    pub enemy_spawns: Vec<EnemySpawn>,
}

/// What can go wrong turning bytes into a [`Level`].
#[derive(Debug)]
pub enum LevelError {
    /// The bytes were not valid UTF-8.
    NotUtf8,
    /// The text was not valid RON, or did not match the schema.
    Syntax(String),
    /// Parsed, but describes an unusable level.
    Invalid(&'static str),
}

impl core::fmt::Display for LevelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LevelError::NotUtf8 => write!(f, "level data is not valid UTF-8"),
            LevelError::Syntax(e) => write!(f, "level data failed to parse: {e}"),
            LevelError::Invalid(e) => write!(f, "level data is invalid: {e}"),
        }
    }
}

impl Level {
    /// Rejects levels that parse but cannot be played.
    ///
    /// Worth doing at the boundary rather than discovering mid-simulation: a
    /// player spawned outside the kill bound would despawn the hero on tick one.
    pub fn validate(&self) -> Result<(), LevelError> {
        if self.world_bounds.half_extents.x <= 0.0 || self.world_bounds.half_extents.y <= 0.0 {
            return Err(LevelError::Invalid(
                "world_bounds has zero or negative extent",
            ));
        }
        if !self.world_bounds.contains(self.player_spawn) {
            return Err(LevelError::Invalid(
                "player_spawn lies outside world_bounds",
            ));
        }
        for s in &self.enemy_spawns {
            if s.toughness <= 0.0 {
                return Err(LevelError::Invalid("enemy toughness must be positive"));
            }
            if s.mass <= 0.0 {
                return Err(LevelError::Invalid("enemy mass must be positive"));
            }
        }
        Ok(())
    }

    /// Parses a level from raw bytes and validates it.
    ///
    /// Takes `&[u8]` rather than a path on purpose. The Shell owns all I/O; on
    /// Android there is no ordinary filesystem path, only APK assets, and the
    /// Core Sim must not know the difference.
    #[cfg(feature = "serde")]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LevelError> {
        let text = core::str::from_utf8(bytes).map_err(|_| LevelError::NotUtf8)?;
        let level: Level = ron::from_str(text).map_err(|e| LevelError::Syntax(e.to_string()))?;
        level.validate()?;
        Ok(level)
    }

    /// Builds a level from an ASCII sketch, for headless tests only.
    ///
    /// Rejected for production per the level format decision, but invaluable for
    /// writing a readable physics test without a RON fixture per case.
    ///
    /// `#` is terrain, `@` is the player spawn, `e` is an Enemy. One character is
    /// one world unit, with row zero at the top.
    #[cfg(any(test, feature = "test-util"))]
    pub fn from_ascii(sketch: &str) -> Self {
        let lines: Vec<&str> = sketch.lines().filter(|l| !l.trim().is_empty()).collect();
        let height = lines.len() as f32;
        let width = lines.iter().map(|l| l.len()).max().unwrap_or(0) as f32;

        let mut static_terrain = Vec::new();
        let mut enemy_spawns = Vec::new();
        let mut player_spawn = Vec2::ZERO;

        for (row, line) in lines.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                // Centre of this cell, with world y growing upward.
                let p = Vec2::new(col as f32 + 0.5, height - row as f32 - 0.5);
                match ch {
                    '#' => static_terrain.push(Aabb::new(p, Vec2::splat(0.5))),
                    '@' => player_spawn = p,
                    'e' => enemy_spawns.push(EnemySpawn {
                        pos: p,
                        toughness: 100.0,
                        mass: 1.0,
                    }),
                    _ => {}
                }
            }
        }

        Level {
            world_bounds: Aabb::new(
                Vec2::new(width * 0.5, height * 0.5),
                Vec2::new(width, height),
            ),
            static_terrain,
            player_spawn,
            enemy_spawns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_sketch_places_everything() {
        let level = Level::from_ascii(
            "
        ....e....
        .........
        ....@....
        #########
        ",
        );
        assert_eq!(level.static_terrain.len(), 9, "bottom row is terrain");
        assert_eq!(level.enemy_spawns.len(), 1);
        assert!(
            level.player_spawn.y < level.enemy_spawns[0].pos.y,
            "enemy sits above the hero"
        );
        level.validate().expect("sketch should be valid");
    }

    #[test]
    fn aabb_bounds_round_trip() {
        let b = Aabb::new(Vec2::new(10.0, 5.0), Vec2::new(2.0, 3.0));
        assert_eq!(b.min(), Vec2::new(8.0, 2.0));
        assert_eq!(b.max(), Vec2::new(12.0, 8.0));
        assert!(b.contains(Vec2::new(10.0, 5.0)));
        assert!(!b.contains(Vec2::new(13.0, 5.0)));
    }

    fn valid() -> Level {
        Level {
            world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(100.0)),
            static_terrain: vec![],
            player_spawn: Vec2::ZERO,
            enemy_spawns: vec![],
        }
    }

    #[test]
    fn rejects_player_spawned_outside_the_kill_bound() {
        // Would despawn the hero on tick one.
        let mut l = valid();
        l.player_spawn = Vec2::new(500.0, 0.0);
        assert!(l.validate().is_err());
    }

    #[test]
    fn rejects_degenerate_world_bounds() {
        let mut l = valid();
        l.world_bounds.half_extents = Vec2::ZERO;
        assert!(l.validate().is_err());
    }

    #[test]
    fn rejects_nonpositive_toughness_and_mass() {
        let mut l = valid();
        l.enemy_spawns = vec![EnemySpawn {
            pos: Vec2::ZERO,
            toughness: 0.0,
            mass: 1.0,
        }];
        assert!(l.validate().is_err());
        l.enemy_spawns = vec![EnemySpawn {
            pos: Vec2::ZERO,
            toughness: 10.0,
            mass: -1.0,
        }];
        assert!(l.validate().is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn parses_ron_with_comments() {
        // The reason RON was chosen over JSON: level files are hand-tuned, and
        // JSON cannot carry a comment explaining a magic number.
        let src = br#"
        Level(
            // the kill bound, deliberately larger than the playfield
            world_bounds: Aabb(centre: (0.0, 0.0), half_extents: (60.0, 40.0)),
            static_terrain: [
                Aabb(centre: (0.0, -10.0), half_extents: (30.0, 1.0)),
            ],
            player_spawn: (0.0, 0.0),
            enemy_spawns: [
                EnemySpawn(pos: (10.0, 0.0), toughness: 150.0, mass: 2.5),
            ],
        )
        "#;
        let level = Level::from_bytes(src).expect("should parse");
        assert_eq!(level.static_terrain.len(), 1);
        assert_eq!(level.enemy_spawns[0].toughness, 150.0);
        assert_eq!(level.world_bounds.half_extents, Vec2::new(60.0, 40.0));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn round_trips_through_ron() {
        let level = Level::from_ascii("..e..\n..@..\n#####\n");
        let text = ron::ser::to_string(&level).expect("serialize");
        let back = Level::from_bytes(text.as_bytes()).expect("deserialize");
        assert_eq!(level, back);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(Level::from_bytes(b"not ron at all").is_err());
        assert!(Level::from_bytes(&[0xff, 0xfe, 0xfd]).is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn validation_runs_on_parse() {
        // Parses cleanly but is unplayable; from_bytes must still reject it.
        let src = br#"
        Level(
            world_bounds: Aabb(centre: (0.0, 0.0), half_extents: (10.0, 10.0)),
            static_terrain: [],
            player_spawn: (900.0, 900.0),
            enemy_spawns: [],
        )
        "#;
        assert!(Level::from_bytes(src).is_err());
    }
}
