//! Events a step emits (native SimEvents). Only substance changes reach
//! the settle's consumers; the rest are recorded for verification.
use crate::sim::SimData;

#[derive(Clone, Debug, Default)]
pub struct Events {
    /// Game-cell indices of SubstanceChangeInfo events, in emission order.
    pub substance_changes: Vec<i32>,
    /// (game cell, mass) of SpawnOre events.
    pub ores: Vec<(i32, f32)>,
    pub melted: Vec<i32>,
    pub backwall_transitions: Vec<i32>,
}

impl Events {
    pub fn clear(&mut self) {
        self.substance_changes.clear();
        self.ores.clear();
        self.melted.clear();
        self.backwall_transitions.clear();
    }

    /// SimEvents::SpawnOre: succeeds for positive mass inside the game area
    /// when forced or when the game cell is visible.
    pub fn spawn_ore(&mut self, d: &SimData, i: usize, mass: f32, force: bool) -> bool {
        if !(mass > 0.0) {
            return false;
        }
        let g = d.game_cell(i);
        if g < 0 || g >= d.num_game_cells {
            return false;
        }
        if !force && d.visible[g as usize] == 0 {
            return false;
        }
        self.ores.push((g, mass));
        true
    }

    /// SimEvents::ChangeSubstance: notifies in-game cells and resets the
    /// cell's stable-tick counter.
    pub fn change_substance(&mut self, d: &mut SimData, i: usize) {
        let g = d.game_cell(i);
        if g >= 0 && g < d.num_game_cells {
            self.substance_changes.push(g);
        }
        d.stable_ticks[i] = 0x1f;
    }

    pub fn backwall_should_transition(&mut self, d: &SimData, i: usize) {
        let g = d.game_cell(i);
        if g >= 0 && g < d.num_game_cells {
            self.backwall_transitions.push(g);
        }
    }

    pub fn cell_melted(&mut self, d: &SimData, i: usize) {
        let g = d.game_cell(i);
        if g >= 0 && g < d.num_game_cells {
            self.melted.push(g);
        }
    }
}
