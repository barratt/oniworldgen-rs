//! Port of `BestFit.BestFitWorlds`. Bin-packs worlds into a 2D grid
//! and writes per-world `(x, y)` offsets. Used during cluster layout
//! (`ProcGenGame.Cluster.InitializeWorlds`) to compute each asteroid's
//! `WorldOffset`.

#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
}

/// C# `BestFit.Rect.X2 = x + width + 2`  (2-cell buffer on the right).
#[inline]
fn x2(p: &Placement) -> i32 { p.x + p.width + 2 }
#[inline]
fn y2(p: &Placement) -> i32 { p.y + p.height + 2 }

fn unoccupied(test: &Placement, placed: &[Placement]) -> bool {
    for p in placed {
        // Same semantics as C#:  a.X1 < b.X2 && a.X2 > b.X1 && a.Y1 < b.Y2 && a.Y2 > b.Y1
        if test.x < x2(p) && x2(test) > p.x && test.y < y2(p) && y2(test) > p.y {
            return false;
        }
    }
    true
}

/// Bin-pack worlds, mutating their `(x, y)`. Returns the final
/// enclosing size (Vector2I in C#). Matches
/// `BestFit.BestFitWorlds(worldsToArrange, ignoreBestFitY = false)`.
///
/// `is_spaced_out` adds the Cluster Space padding (+136 x, max y 136)
/// matching `DlcManager.FeatureClusterSpaceEnabled()` in C#.
pub fn best_fit(worlds: &mut [Placement], is_spaced_out: bool) -> (i32, i32) {
    if worlds.is_empty() {
        return (0, 0);
    }

    // C# List<T>.Sort uses Array.Sort → introsort, which for small
    // inputs (≤16) falls through to insertion sort. That's stable.
    // Use Rust's stable sort_by to match.
    let mut order: Vec<usize> = (0..worlds.len()).collect();
    order.sort_by(|&a, &b| worlds[b].height.cmp(&worlds[a].height));

    let tallest_h = worlds[order[0]].height;
    let mut placed: Vec<Placement> = Vec::with_capacity(worlds.len());
    let mut bound_x = 0i32;
    let mut bound_y = 0i32;

    for &idx in &order {
        let w = worlds[idx].width;
        let h = worlds[idx].height;
        let mut pos = (0i32, 0i32);
        loop {
            let test = Placement { width: w, height: h, x: pos.0, y: pos.1 };
            if unoccupied(&test, &placed) { break; }
            if pos.1 + h >= tallest_h + 32 {
                pos.1 = 0;
                pos.0 += 1;
            } else {
                pos.1 += 1;
            }
        }
        bound_x = bound_x.max(w + pos.0);
        bound_y = bound_y.max(h + pos.1);
        placed.push(Placement { width: w, height: h, x: pos.0, y: pos.1 });
        worlds[idx].x = pos.0;
        worlds[idx].y = pos.1;
    }

    if is_spaced_out {
        bound_x += 136;
        bound_y = bound_y.max(136);
    }
    (bound_x, bound_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_world_at_origin() {
        let mut w = [Placement { width: 256, height: 384, x: 0, y: 0 }];
        let (bx, by) = best_fit(&mut w, false);
        assert_eq!(w[0].x, 0);
        assert_eq!(w[0].y, 0);
        assert_eq!((bx, by), (256, 384));
    }

    #[test]
    fn two_worlds_stack() {
        // Two small worlds, tallest first.
        let mut w = [
            Placement { width: 100, height: 200, x: 0, y: 0 },
            Placement { width: 100, height: 50, x: 0, y: 0 },
        ];
        best_fit(&mut w, false);
        assert_eq!((w[0].x, w[0].y), (0, 0));
        // Second world stacked right after first (x=102 because of +2 buffer).
        assert_eq!((w[1].x, w[1].y), (102, 0));
    }

    /// V-SNDST-C: SpacedOut Classic Terra has 8 worlds. Compare against
    /// the per-world BestFit placement used by ONI. Expected offsets
    /// derived from the game's decompiled C# BestFit run by hand
    /// (sort by height desc, stable, then bin-pack).
    #[test]
    fn v_sndst_c_layout() {
        let mut worlds = [
            Placement { width: 240, height: 380, x: 0, y: 0 }, // VanillaSandstoneDefault
            Placement { width: 160, height: 176, x: 0, y: 0 }, // MedRadioactiveWarpPlanet
            Placement { width:  64, height: 128, x: 0, y: 0 }, // TundraMoonlet
            Placement { width:  64, height:  96, x: 0, y: 0 }, // MarshyMoonlet
            Placement { width:  64, height:  96, x: 0, y: 0 }, // NiobiumMoonlet
            Placement { width:  96, height:  80, x: 0, y: 0 }, // MooMoonlet
            Placement { width:  80, height: 174, x: 0, y: 0 }, // WaterMoonlet
            Placement { width:  96, height:  96, x: 0, y: 0 }, // MiniRegolithMoonlet
        ];
        best_fit(&mut worlds, true);
        // VanSD at origin (tallest).
        assert_eq!((worlds[0].x, worlds[0].y), (0, 0));
        // MedRad right of VanSD (+2 buffer = 242).
        assert_eq!((worlds[1].x, worlds[1].y), (242, 0));
    }
}
