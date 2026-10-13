//! Grid state: the per-cell structure-of-arrays and the simulation data.

/// Per-cell arrays (native CellSOA).
#[derive(Clone, Debug, Default)]
pub struct Cells {
    pub element: Vec<u16>,
    pub temperature: Vec<f32>,
    pub mass: Vec<f32>,
    pub properties: Vec<u8>,
    pub insulation: Vec<u8>,
    pub strength: Vec<u8>,
    pub disease: Vec<u8>,
    pub disease_count: Vec<i32>,
    pub infestation_ticks: Vec<u8>,
    pub growth_error: Vec<f32>,
    pub radiation: Vec<f32>,
}

impl Cells {
    /// Freshly allocated grid with the native constructor's defaults.
    pub fn new(n: usize) -> Self {
        Cells {
            element: vec![0xffff; n],
            temperature: vec![0.0; n],
            mass: vec![0.0; n],
            properties: vec![0; n],
            insulation: vec![0xff; n],
            strength: vec![0x84; n],
            disease: vec![0xff; n],
            disease_count: vec![0; n],
            infestation_ticks: vec![0; n],
            growth_error: vec![0.0; n],
            radiation: vec![0.0; n],
        }
    }
    pub fn copy_from(&mut self, o: &Cells) {
        self.element.copy_from_slice(&o.element);
        self.temperature.copy_from_slice(&o.temperature);
        self.mass.copy_from_slice(&o.mass);
        self.properties.copy_from_slice(&o.properties);
        self.insulation.copy_from_slice(&o.insulation);
        self.strength.copy_from_slice(&o.strength);
        self.disease.copy_from_slice(&o.disease);
        self.disease_count.copy_from_slice(&o.disease_count);
        self.infestation_ticks.copy_from_slice(&o.infestation_ticks);
        self.growth_error.copy_from_slice(&o.growth_error);
        self.radiation.copy_from_slice(&o.radiation);
    }
}

/// Backwall layer (native BackwallSOA).
#[derive(Clone, Debug, Default)]
pub struct Backwall {
    pub vacuum_element: u16,
    pub element: Vec<u16>,
    pub mass: Vec<f32>,
    pub temperature: Vec<f32>,
}

impl Backwall {
    pub fn clear(&mut self, i: usize) {
        self.element[i] = self.vacuum_element;
        self.mass[i] = 0.0;
        self.temperature[i] = 0.0;
    }
}

/// CellSOA::SwapCells: element, mass, temperature and germ fields.
pub fn swap_cells(c: &mut Cells, a: usize, b: usize) {
    c.element.swap(a, b);
    c.mass.swap(a, b);
    c.temperature.swap(a, b);
    c.disease.swap(a, b);
    c.disease_count.swap(a, b);
    c.infestation_ticks.swap(a, b);
    c.growth_error.swap(a, b);
}
