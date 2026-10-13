//! LibNoiseDotNet primitives used by ONI worldgen.
//!
//! Ported from the LibNoiseDotNet C# library. `ImprovedPerlin` and
//! `SimplexPerlin` are the two used by ONI's noise tree.
//!
//! ONI runs these C# functions under Mono. Float locals and arguments round
//! to f32, while compound expressions retain double precision. Explicit f64
//! intermediates below reproduce that behavior, verified with live game probes.

#![allow(non_snake_case, clippy::many_single_char_names)]

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NoiseQuality {
    Fast,
    Standard,
    Best,
}

#[inline]
pub fn s_curve3(a: f32) -> f32 {
    let a = a as f64;
    (a * a * (3.0 - 2.0 * a)) as f32
}

#[inline]
pub fn s_curve5(a: f32) -> f32 {
    let a = a as f64;
    (a * a * a * (a * (a * 6.0 - 15.0) + 10.0)) as f32
}

#[inline]
pub fn lerp(n0: f32, n1: f32, a: f32) -> f32 {
    (n0 as f64 + a as f64 * (n1 as f64 - n0 as f64)) as f32
}

#[inline]
pub fn fast_floor(x: f32) -> i32 {
    if !(x >= 0.0) { (x as i32) - 1 } else { x as i32 }
}

const SOURCE: [i32; 256] = [
    151,160,137,91,90,15,131,13,201,95,
    96,53,194,233,7,225,140,36,103,30,
    69,142,8,99,37,240,21,10,23,190,
    6,148,247,120,234,75,0,26,197,62,
    94,252,219,203,117,35,11,32,57,177,
    33,88,237,149,56,87,174,20,125,136,
    171,168,68,175,74,165,71,134,139,48,
    27,166,77,146,158,231,83,111,229,122,
    60,211,133,230,220,105,92,41,55,46,
    245,40,244,102,143,54,65,25,63,161,
    1,216,80,73,209,76,132,187,208,89,
    18,169,200,196,135,130,116,188,159,86,
    164,100,109,198,173,186,3,64,52,217,
    226,250,124,123,5,202,38,147,118,126,
    255,82,85,212,207,206,59,227,47,16,
    58,17,182,189,28,42,223,183,170,213,
    119,248,152,2,44,154,163,70,221,153,
    101,155,167,43,172,9,129,22,39,253,
    19,98,108,110,79,113,224,232,178,185,
    112,104,218,246,97,228,251,34,242,193,
    238,210,144,12,191,179,162,241,81,51,
    145,235,249,14,239,107,49,192,214,31,
    181,199,106,157,184,84,204,176,115,121,
    50,45,127,4,150,254,138,236,205,93,
    222,114,67,29,24,72,243,141,128,195,
    78,66,215,61,156,180,
];

fn permute_256(seed: i32) -> [i32; 512] {
    let mut r = [0i32; 512];
    if seed != 0 {
        let b = [
            (seed & 0xff) as u8,
            ((seed >> 8) & 0xff) as u8,
            ((seed >> 16) & 0xff) as u8,
            ((seed >> 24) & 0xff) as u8,
        ];
        for i in 0..256 {
            let mut v = SOURCE[i] ^ b[0] as i32;
            v ^= b[1] as i32;
            v ^= b[2] as i32;
            v ^= b[3] as i32;
            r[i] = v;
            r[i + 256] = v;
        }
    } else {
        for i in 0..256 {
            r[i] = SOURCE[i];
            r[i + 256] = SOURCE[i];
        }
    }
    r
}

pub struct ImprovedPerlin {
    pub seed: i32,
    pub quality: NoiseQuality,
    random: [i32; 512],
}

impl ImprovedPerlin {
    pub fn new(seed: i32, quality: NoiseQuality) -> Self {
        Self { seed, quality, random: permute_256(seed) }
    }

    #[inline]
    fn grad3(hash: i32, x: f32, y: f32, z: f32) -> f32 {
        let h = hash & 0xf;
        let u = if h < 8 { x } else { y };
        let v = if h < 4 { y } else if h == 12 || h == 14 { x } else { z };
        (if (h & 1) == 0 { u } else { -u }) + (if (h & 2) == 0 { v } else { -v })
    }

    pub fn get_value_3(&self, mut x: f32, mut y: f32, mut z: f32) -> f32 {
        let xi = if (x as f64) > 0.0 { x as i32 } else { x as i32 - 1 };
        let yi = if (y as f64) > 0.0 { y as i32 } else { y as i32 - 1 };
        let zi = if (z as f64) > 0.0 { z as i32 } else { z as i32 - 1 };
        let xa = (xi & 0xff) as usize;
        let ya = (yi & 0xff) as usize;
        let za = (zi & 0xff) as usize;
        x -= xi as f32;
        y -= yi as f32;
        z -= zi as f32;
        let (a, b, c) = match self.quality {
            NoiseQuality::Fast => (x, y, z),
            NoiseQuality::Standard => (s_curve3(x), s_curve3(y), s_curve3(z)),
            NoiseQuality::Best => (s_curve5(x), s_curve5(y), s_curve5(z)),
        };
        // Table entries are 0..=255, so every index below is under 512; the
        // masks only let the compiler drop the bounds checks.
        let r = &self.random;
        let at = |i: usize| r[i & 511];
        let p_xy = (at(xa) + ya as i32) as usize;
        let p_xyz = (at(p_xy) + za as i32) as usize;
        let p_xyz1 = (at(p_xy + 1) + za as i32) as usize;
        let p_x1y = (at(xa + 1) + ya as i32) as usize;
        let p_x1yz = (at(p_x1y) + za as i32) as usize;
        let p_x1yz1 = (at(p_x1y + 1) + za as i32) as usize;
        lerp(
            lerp(
                lerp(Self::grad3(at(p_xyz), x, y, z), Self::grad3(at(p_x1yz), x - 1.0, y, z), a),
                lerp(Self::grad3(at(p_xyz1), x, y - 1.0, z), Self::grad3(at(p_x1yz1), x - 1.0, y - 1.0, z), a),
                b,
            ),
            lerp(
                lerp(Self::grad3(at(p_xyz + 1), x, y, z - 1.0), Self::grad3(at(p_x1yz + 1), x - 1.0, y, z - 1.0), a),
                lerp(Self::grad3(at(p_xyz1 + 1), x, y - 1.0, z - 1.0), Self::grad3(at(p_x1yz1 + 1), x - 1.0, y - 1.0, z - 1.0), a),
                b,
            ),
            c,
        )
    }
}

// --- SimplexPerlin ---

const F2: f32 = 0.366_025_4;
const G2: f32 = 0.211_324_87;
#[allow(dead_code)]
const F3: f32 = 1.0 / 3.0;
const G3: f32 = 1.0 / 6.0;

const GRAD3: [[i32; 3]; 12] = [
    [ 1, 1, 0], [-1, 1, 0], [ 1,-1, 0], [-1,-1, 0],
    [ 1, 0, 1], [-1, 0, 1], [ 1, 0,-1], [-1, 0,-1],
    [ 0, 1, 1], [ 0,-1, 1], [ 0, 1,-1], [ 0,-1,-1],
];

pub struct SimplexPerlin {
    pub seed: i32,
    random: [i32; 512],
    /// `random[i] % 12`, the gradient index (entries are 0..=255).
    gradient: [u8; 512],
}

impl SimplexPerlin {
    pub fn new(seed: i32) -> Self {
        let random = permute_256(seed);
        Self { seed, random, gradient: std::array::from_fn(|i| (random[i] % 12) as u8) }
    }

    #[inline]
    fn dot3(g: [i32; 3], x: f32, y: f32, z: f32) -> f32 {
        (g[0] as f64 * x as f64 + g[1] as f64 * y as f64 + g[2] as f64 * z as f64) as f32
    }
    #[inline]
    fn dot2(g: [i32; 3], x: f32, y: f32) -> f32 {
        g[0] as f32 * x + g[1] as f32 * y
    }

    pub fn get_value_2(&self, x: f32, y: f32) -> f32 {
        let s = (x + y) * F2;
        let i = fast_floor(x + s);
        let j = fast_floor(y + s);
        let t = (i + j) as f32 * G2;
        let x0 = x - (i as f32 - t);
        let y0 = y - (j as f32 - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - i1 as f32 + G2;
        let y1 = y0 - j1 as f32 + G2;
        let g22: f32 = G2 * 2.0 - 1.0;
        let x2 = x0 + g22;
        let y2 = y0 + g22;
        let ii = (i & 0xff) as usize;
        let jj = (j & 0xff) as usize;
        let r = &self.random;
        let mut n0 = 0.0f32;
        let t0 = 0.5 - x0 * x0 - y0 * y0;
        if t0 > 0.0 {
            let t0 = t0 * t0;
            let idx = (r[ii + (r[jj] as usize)] % 12) as usize;
            n0 = t0 * t0 * Self::dot2(GRAD3[idx], x0, y0);
        }
        let mut n1 = 0.0f32;
        let t1 = 0.5 - x1 * x1 - y1 * y1;
        if t1 > 0.0 {
            let t1 = t1 * t1;
            let idx = (r[ii + i1 + (r[jj + j1] as usize)] % 12) as usize;
            n1 = t1 * t1 * Self::dot2(GRAD3[idx], x1, y1);
        }
        let mut n2 = 0.0f32;
        let t2 = 0.5 - x2 * x2 - y2 * y2;
        if t2 > 0.0 {
            let t2 = t2 * t2;
            let idx = (r[ii + 1 + (r[jj + 1] as usize)] % 12) as usize;
            n2 = t2 * t2 * Self::dot2(GRAD3[idx], x2, y2);
        }
        70.0 * (n0 + n1 + n2)
    }

    pub fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        let s = ((x as f64 + y as f64 + z as f64) * F3 as f64) as f32;
        let i = fast_floor(x + s);
        let j = fast_floor(y + s);
        let k = fast_floor(z + s);
        let t = (i + j + k) as f32 * G3;
        let x0 = (x as f64 - (i as f64 - t as f64)) as f32;
        let y0 = (y as f64 - (j as f64 - t as f64)) as f32;
        let z0 = (z as f64 - (k as f64 - t as f64)) as f32;
        let (i1, j1, k1, i2, j2, k2);
        if x0 >= y0 {
            if y0 >= z0 { i1=1;j1=0;k1=0; i2=1;j2=1;k2=0; }
            else if x0 >= z0 { i1=1;j1=0;k1=0; i2=1;j2=0;k2=1; }
            else { i1=0;j1=0;k1=1; i2=1;j2=0;k2=1; }
        } else if y0 < z0 { i1=0;j1=0;k1=1; i2=0;j2=1;k2=1; }
        else if x0 < z0 { i1=0;j1=1;k1=0; i2=0;j2=1;k2=1; }
        else { i1=0;j1=1;k1=0; i2=1;j2=1;k2=0; }
        let x1 = (x0 as f64 - i1 as f64 + G3 as f64) as f32;
        let y1 = (y0 as f64 - j1 as f64 + G3 as f64) as f32;
        let z1 = (z0 as f64 - k1 as f64 + G3 as f64) as f32;
        let x2 = (x0 as f64 - i2 as f64 + F3 as f64) as f32;
        let y2 = (y0 as f64 - j2 as f64 + F3 as f64) as f32;
        let z2 = (z0 as f64 - k2 as f64 + F3 as f64) as f32;
        let x3 = x0 - 0.5;
        let y3 = y0 - 0.5;
        let z3 = z0 - 0.5;
        let ii = (i & 0xff) as usize;
        let jj = (j & 0xff) as usize;
        let kk = (k & 0xff) as usize;
        // Table entries are 0..=255, so every index below is under 512; the
        // masks only let the compiler drop the bounds checks.
        let r = |i: usize| self.random[i & 511] as usize;
        let g = |i: usize| self.gradient[i & 511] as usize;
        let mut n0 = 0.0f32;
        let t0 = simplex_kernel(x0, y0, z0);
        if t0 > 0.0 {
            let t0 = t0 * t0;
            let idx = g(ii + r(jj + r(kk)));
            n0 = (t0 as f64 * t0 as f64 * Self::dot3(GRAD3[idx], x0, y0, z0) as f64) as f32;
        }
        let mut n1 = 0.0f32;
        let t1 = simplex_kernel(x1, y1, z1);
        if t1 > 0.0 {
            let t1 = t1 * t1;
            let idx = g(ii + i1 + r(jj + j1 + r(kk + k1)));
            n1 = (t1 as f64 * t1 as f64 * Self::dot3(GRAD3[idx], x1, y1, z1) as f64) as f32;
        }
        let mut n2 = 0.0f32;
        let t2 = simplex_kernel(x2, y2, z2);
        if t2 > 0.0 {
            let t2 = t2 * t2;
            let idx = g(ii + i2 + r(jj + j2 + r(kk + k2)));
            n2 = (t2 as f64 * t2 as f64 * Self::dot3(GRAD3[idx], x2, y2, z2) as f64) as f32;
        }
        let mut n3 = 0.0f32;
        let t3 = simplex_kernel(x3, y3, z3);
        if t3 > 0.0 {
            let t3 = t3 * t3;
            let idx = g(ii + 1 + r(jj + 1 + r(kk + 1)));
            n3 = (t3 as f64 * t3 as f64 * Self::dot3(GRAD3[idx], x3, y3, z3) as f64) as f32;
        }
        (32.0 * (n0 as f64 + n1 as f64 + n2 as f64 + n3 as f64)) as f32
    }
}

// ---------------- Simple primitives ----------------

pub struct BevinsGradient { pub seed:i32, pub quality:NoiseQuality }
impl Source3D for BevinsGradient {
    fn get_value_3(&self,x:f32,y:f32,z:f32)->f32 {
        let floor = |v:f32| if v>0.0 {v as i32} else {v as i32-1};
        let (ix,iy,iz)=(floor(x),floor(y),floor(z));
        let curve = |v| match self.quality {NoiseQuality::Fast=>v,NoiseQuality::Standard=>s_curve3(v),NoiseQuality::Best=>s_curve5(v)};
        let (a,b,c)=(curve(x-ix as f32),curve(y-iy as f32),curve(z-iz as f32));
        let gradient=|i:i32,j:i32,k:i32| {
            let h=i.wrapping_mul(1619).wrapping_add(j.wrapping_mul(31337)).wrapping_add(k.wrapping_mul(6971));
            let h=(h as i64 + 1013_i64*self.seed as i64)&0xffff_ffff;
            let vector=crate::gradient_vectors::VECTORS[((h^(h>>8))&255) as usize];
            let (dx,dy,dz)=(x-i as f32,y-j as f32,z-k as f32);
            ((vector[0] as f64*dx as f64+vector[1] as f64*dy as f64+vector[2] as f64*dz as f64)*0.57735026_f32 as f64) as f32
        };
        lerp(lerp(lerp(gradient(ix,iy,iz),gradient(ix+1,iy,iz),a),lerp(gradient(ix,iy+1,iz),gradient(ix+1,iy+1,iz),a),b),
            lerp(lerp(gradient(ix,iy,iz+1),gradient(ix+1,iy,iz+1),a),lerp(gradient(ix,iy+1,iz+1),gradient(ix+1,iy+1,iz+1),a),b),c)
    }
}

fn simplex_kernel(x: f32, y: f32, z: f32) -> f32 {
    let (x, y, z) = (x as f64, y as f64, z as f64);
    (0.6_f32 as f64 - x*x - y*y - z*z) as f32
}

pub struct Constant(pub f32);
impl Constant {
    #[inline]
    pub fn get_value_3(&self, _x: f32, _y: f32, _z: f32) -> f32 { self.0 }
}

pub struct Cylinders { pub frequency: f32 }
impl Cylinders {
    pub fn get_value_3(&self, mut x: f32, _y: f32, mut z: f32) -> f32 {
        x *= self.frequency; z *= self.frequency;
        let d = (x * x + z * z).sqrt();
        let f = d - d.floor();
        let v = 1.0 - f;
        let m = f.min(v);
        1.0 - m * 4.0
    }
}

/// BevinsValue: value-noise primitive. 64-bit integer hash lattice,
/// trilerped with SCurve smoothing.
pub struct BevinsValue { pub seed: i32, pub quality: NoiseQuality }

impl BevinsValue {
    pub fn new(seed: i32, quality: NoiseQuality) -> Self { Self { seed, quality } }
    pub fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        value_coherent_noise_3d(x, y, z, self.seed as i64, self.quality)
    }
}

fn int_value_noise_3d(x: i32, y: i32, z: i32, seed: i64) -> i32 {
    let n = (1619i64.wrapping_mul(x as i64)
        .wrapping_add(31337i64.wrapping_mul(y as i64))
        .wrapping_add(6971i64.wrapping_mul(z as i64))
        .wrapping_add(1013i64.wrapping_mul(seed))) & 0x7FFFFFFF;
    let n = (n >> 13) ^ n;
    let a = n.wrapping_mul(n).wrapping_mul(60493).wrapping_add(19990303);
    (n.wrapping_mul(a).wrapping_add(1376312589) & 0x7FFFFFFF) as i32
}

fn value_noise_3d(x: i32, y: i32, z: i32, seed: i64) -> f32 {
    // conv.r4 rounds the integer first; Mono then evaluates division and
    // subtraction in double precision before rounding the return value.
    (1.0 - int_value_noise_3d(x, y, z, seed) as f32 as f64 / 1_073_741_824.0) as f32
}

fn value_coherent_noise_3d(x: f32, y: f32, z: f32, seed: i64, q: NoiseQuality) -> f32 {
    let xi = if (x as f64) > 0.0 { x as i32 } else { x as i32 - 1 };
    let yi = if (y as f64) > 0.0 { y as i32 } else { y as i32 - 1 };
    let zi = if (z as f64) > 0.0 { z as i32 } else { z as i32 - 1 };
    let (a, b, c) = match q {
        NoiseQuality::Fast => (x - xi as f32, y - yi as f32, z - zi as f32),
        NoiseQuality::Standard => (
            s_curve3(x - xi as f32), s_curve3(y - yi as f32), s_curve3(z - zi as f32)),
        NoiseQuality::Best => (
            s_curve5(x - xi as f32), s_curve5(y - yi as f32), s_curve5(z - zi as f32)),
    };
    let n00 = value_noise_3d(xi,    yi,    zi,    seed);
    let n10 = value_noise_3d(xi+1, yi,    zi,    seed);
    let n01 = value_noise_3d(xi,    yi+1, zi,    seed);
    let n11 = value_noise_3d(xi+1, yi+1, zi,    seed);
    let n02 = value_noise_3d(xi,    yi,    zi+1, seed);
    let n12 = value_noise_3d(xi+1, yi,    zi+1, seed);
    let n03 = value_noise_3d(xi,    yi+1, zi+1, seed);
    let n13 = value_noise_3d(xi+1, yi+1, zi+1, seed);
    let l0 = lerp(lerp(n00, n10, a), lerp(n01, n11, a), b);
    let l1 = lerp(lerp(n02, n12, a), lerp(n03, n13, a), b);
    lerp(l0, l1, c)
}

pub struct Spheres { pub frequency: f32 }
impl Spheres {
    pub fn get_value_3(&self, mut x: f32, mut y: f32, mut z: f32) -> f32 {
        x *= self.frequency; y *= self.frequency; z *= self.frequency;
        let d = (x * x + y * y + z * z).sqrt();
        let f = d - d.floor();
        let v = 1.0 - f;
        let m = f.min(v);
        1.0 - m * 4.0
    }
}

// ---------------- Filters (multi-octave on top of a primitive) ----------------
//
// Each filter wraps a `Source3D` (our primitive trait) and applies
// multi-octave combination with spectral weighting. Ported from the
// LibNoiseDotNet C# library.

pub trait Source3D: Send + Sync {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32;
}

impl Source3D for ImprovedPerlin { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value_3(x, y, z) } }
impl Source3D for SimplexPerlin  { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value_3(x, y, z) } }
impl Source3D for Constant       { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value_3(x, y, z) } }
impl Source3D for Cylinders      { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value_3(x, y, z) } }
impl Source3D for Spheres        { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value_3(x, y, z) } }
impl Source3D for BevinsValue    { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value_3(x, y, z) } }

pub struct SpectralWeights([f32; 30]);
impl SpectralWeights {
    pub fn new(lacunarity: f32, exponent: f32) -> Self {
        let mut w = [0f32; 30];
        for i in 0..30 {
            w[i] = (lacunarity as f64).powf(-(i as f64) * exponent as f64) as f32;
        }
        SpectralWeights(w)
    }
}

pub struct RidgedMultiFractal {
    pub frequency: f32,
    pub lacunarity: f32,
    pub octave_count: f32,
    pub offset: f32,
    pub gain: f32,
    pub spectral: SpectralWeights,
    pub source: Box<dyn Source3D>,
}

impl RidgedMultiFractal {
    pub fn get_value(&self, mut x: f32, mut y: f32, mut z: f32) -> f32 {
        x *= self.frequency; y *= self.frequency; z *= self.frequency;
        let mut v = self.source.get_value_3(x, y, z);
        if (v as f64) < 0.0 { v = -v; }
        v = self.offset - v;
        v *= v;
        let mut total = v;
        let mut weight = 1.0f32;
        let mut i = 1usize;
        while (weight as f64) > 0.001 && (i as f32) < self.octave_count {
            x *= self.lacunarity; y *= self.lacunarity; z *= self.lacunarity;
            weight = (v * self.gain).clamp(0.0, 1.0);
            v = self.source.get_value_3(x, y, z);
            if v < 0.0 { v = -v; }
            v = self.offset - v;
            v *= v;
            v *= weight;
            total = (total as f64 + v as f64 * self.spectral.0[i] as f64) as f32;
            i += 1;
        }
        total
    }
}

pub struct Billow {
    pub frequency: f32,
    pub lacunarity: f32,
    pub octave_count: f32,
    pub scale: f32,
    pub bias: f32,
    pub spectral: SpectralWeights,
    pub source: Box<dyn Source3D>,
}

impl Billow {
    pub fn get_value(&self, mut x: f32, mut y: f32, mut z: f32) -> f32 {
        x *= self.frequency; y *= self.frequency; z *= self.frequency;
        let mut sum = 0.0f32;
        let mut i: usize = 0;
        while (i as f32) < self.octave_count {
            let mut v = self.source.get_value_3(x, y, z) * self.spectral.0[i];
            if v < 0.0 { v = -v; }
            sum = (sum as f64 + v as f64 * self.scale as f64 + self.bias as f64) as f32;
            x *= self.lacunarity; y *= self.lacunarity; z *= self.lacunarity;
            i += 1;
        }
        let frac = self.octave_count - (self.octave_count as i32 as f32);
        if frac > 0.0 {
            sum = (sum as f64 + self.scale as f64 * frac as f64 * self.source.get_value_3(x, y, z) as f64 * self.spectral.0[i] as f64 + self.bias as f64) as f32;
        }
        sum
    }
}

pub struct HybridMultiFractal {
    pub frequency: f32,
    pub lacunarity: f32,
    pub octave_count: f32,
    pub offset: f32,
    pub gain: f32,
    pub spectral: SpectralWeights,
    pub source: Box<dyn Source3D>,
}

impl HybridMultiFractal {
    pub fn get_value(&self, mut x: f32, mut y: f32, mut z: f32) -> f32 {
        x *= self.frequency; y *= self.frequency; z *= self.frequency;
        let mut total = self.source.get_value_3(x, y, z) + self.offset;
        let mut weight = self.gain * total;
        x *= self.lacunarity; y *= self.lacunarity; z *= self.lacunarity;
        let mut i: usize = 1;
        while (weight as f64) > 0.001 && (i as f32) < self.octave_count {
            if (weight as f64) > 1.0 { weight = 1.0; }
            // Mono evaluates `(_offset + GetValue) * w[i]` and `num2 *= _gain * num4`
            // on its double evaluation stack, rounding only at the store (DLC4 Garden noise).
            let mut signal = ((self.offset as f64 + self.source.get_value_3(x, y, z) as f64) * self.spectral.0[i] as f64) as f32;
            signal *= weight;
            total += signal;
            weight = (weight as f64 * (self.gain as f64 * signal as f64)) as f32;
            x *= self.lacunarity; y *= self.lacunarity; z *= self.lacunarity;
            i += 1;
        }
        let frac = self.octave_count - (self.octave_count as i32 as f32);
        if frac > 0.0 {
            let mut signal = self.source.get_value_3(x, y, z);
            signal *= self.spectral.0[i];
            signal *= frac;
            total += signal;
        }
        total
    }
}

pub struct SinFractal {
    pub frequency: f32,
    pub lacunarity: f32,
    pub octave_count: f32,
    pub spectral: SpectralWeights,
    pub source: Box<dyn Source3D>,
}

impl SinFractal {
    pub fn get_value(&self, x_in: f32, mut y: f32, mut z: f32) -> f32 {
        let x_saved = x_in;
        let mut x = x_in * self.frequency;
        y *= self.frequency; z *= self.frequency;
        let mut sum = 0.0f32;
        let mut i: usize = 0;
        while (i as f32) < self.octave_count {
            let mut v = self.source.get_value_3(x, y, z) * self.spectral.0[i];
            if (v as f64) < 0.0 { v = -v; }
            sum += v;
            x *= self.lacunarity; y *= self.lacunarity; z *= self.lacunarity;
            i += 1;
        }
        let frac = self.octave_count - (self.octave_count as i32 as f32);
        if frac > 0.0 {
            sum = (sum as f64 + frac as f64 * self.source.get_value_3(x, y, z) as f64 * self.spectral.0[i] as f64) as f32;
        }
        (x_saved as f64 + sum as f64).sin() as f32
    }
}

pub struct VoronoiFilter {
    pub frequency: f32,
    pub displacement: f32,
    pub distance: bool,
    pub source: Box<dyn Source3D>,
    pub lattice: LatticeCache,
}

/// Source values at the 5x5x5 lattice points around the previous sample's
/// cell. Sources are pure functions of their input, so neighbouring samples
/// in the same cell reuse them instead of re-evaluating 125 points; moving
/// to a neighbouring cell takes the points the blocks share from a
/// direct-mapped table of recent lattice points.
#[derive(Default)]
pub struct LatticeCache(std::sync::Mutex<LatticeState>);

#[derive(Default)]
pub struct LatticeState { block: Option<((i32, i32, i32), [f32; 125])>, points: Vec<Option<((i32, i32, i32), f32)>> }

const LATTICE_TABLE: usize = 1 << 12;

impl VoronoiFilter {
    pub fn get_value(&self, mut x: f32, mut y: f32, mut z: f32) -> f32 {
        x *= self.frequency; y *= self.frequency; z *= self.frequency;
        let ix = if x > 0.0 { x as i32 } else { x as i32 - 1 };
        let iy = if y > 0.0 { y as i32 } else { y as i32 - 1 };
        let iz = if z > 0.0 { z as i32 } else { z as i32 - 1 };
        let mut best = 2_147_483_600.0f32;
        let mut bx = 0.0f32; let mut by = 0.0f32; let mut bz = 0.0f32;
        let mut guard = self.lattice.0.lock().unwrap_or_else(|e| e.into_inner());
        let cache = &mut *guard;
        if cache.block.as_ref().map(|c| c.0) != Some((ix, iy, iz)) {
            if cache.points.is_empty() { cache.points = vec![None; LATTICE_TABLE]; }
            let mut values = [0.0f32; 125];
            let mut n = 0;
            for i in (iz - 2)..=(iz + 2) {
                for j in (iy - 2)..=(iy + 2) {
                    for k in (ix - 2)..=(ix + 2) {
                        let slot = (k.wrapping_mul(73_856_093) ^ j.wrapping_mul(19_349_663) ^ i.wrapping_mul(83_492_791)) as u32 as usize % LATTICE_TABLE;
                        values[n] = match cache.points[slot] {
                            Some((key, value)) if key == (k, j, i) => value,
                            _ => {
                                let value = self.source.get_value_3(k as f32, j as f32, i as f32);
                                cache.points[slot] = Some(((k, j, i), value));
                                value
                            }
                        };
                        n += 1;
                    }
                }
            }
            cache.block = Some(((ix, iy, iz), values));
        }
        let values = &cache.block.as_ref().unwrap().1;
        let mut n = 0;
        for i in (iz - 2)..=(iz + 2) {
            for j in (iy - 2)..=(iy + 2) {
                for k in (ix - 2)..=(ix + 2) {
                    // The game evaluates the source three times with these
                    // same arguments.
                    let v = values[n];
                    n += 1;
                    let sx = k as f32 + v;
                    let sy = j as f32 + v;
                    let sz = i as f32 + v;
                    let dx = sx - x; let dy = sy - y; let dz = sz - z;
                    // Mono retains double intermediates until the local float
                    // assignment. Rounding each multiply/add can select a
                    // different lattice point for nearly equal distances.
                    let d = (dx as f64*dx as f64 + dy as f64*dy as f64 + dz as f64*dz as f64) as f32;
                    if d < best {
                        best = d; bx = sx; by = sy; bz = sz;
                    }
                }
            }
        }
        let v = if self.distance {
            let dx = bx - x; let dy = by - y; let dz = bz - z;
            (dx*dx + dy*dy + dz*dz).sqrt() * 1.7320508 - 1.0
        } else {
            0.0
        };
        v + self.displacement * self.source.get_value_3(bx.floor(), by.floor(), bz.floor())
    }
}

// ---------------- Filters implement Source3D (so they can chain) ----------------

impl Source3D for RidgedMultiFractal { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value(x, y, z) } }
impl Source3D for Billow             { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value(x, y, z) } }
impl Source3D for HybridMultiFractal { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value(x, y, z) } }
impl Source3D for SinFractal         { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value(x, y, z) } }
impl Source3D for VoronoiFilter      { fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 { self.get_value(x, y, z) } }

// ---------------- Modifiers (single-source transforms) ----------------

pub struct ScaleBias {
    pub scale: f32,
    pub bias: f32,
    pub source: Box<dyn Source3D>,
}
impl Source3D for ScaleBias {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        (self.source.get_value_3(x, y, z) as f64 * self.scale as f64 + self.bias as f64) as f32
    }
}

pub struct Clamp {
    pub lower: f32,
    pub upper: f32,
    pub source: Box<dyn Source3D>,
}
impl Source3D for Clamp {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        let v = self.source.get_value_3(x, y, z);
        if v < self.lower { self.lower } else if v > self.upper { self.upper } else { v }
    }
}

pub struct Invert {
    pub source: Box<dyn Source3D>,
}
impl Source3D for Invert {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        -self.source.get_value_3(x, y, z)
    }
}

/// Scale2d modifier: `GetValue(x*scale.x, y, z*scale.y)`
pub struct Scale2d {
    pub scale_x: f32,
    pub scale_y: f32,   // NB: stored in `z` slot of Vector2.y per C#
    pub source: Box<dyn Source3D>,
}
impl Source3D for Scale2d {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        self.source.get_value_3(x * self.scale_x, y, z * self.scale_y)
    }
}

/// Generic passthrough transformer — used when the transformer type
/// isn't recognized. Wraps the source unchanged so the tree still
/// builds.
pub struct TransformPassthrough {
    pub source: Box<dyn Source3D>,
}
impl Source3D for TransformPassthrough {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        self.source.get_value_3(x, y, z)
    }
}

/// RotatePoint — 3D rotation of input vector before sampling source.
/// Matches `LibNoiseDotNet.Graphics.Tools.Noise.Tranformer.RotatePoint`.
pub struct RotatePoint {
    pub m: [[f32; 3]; 3],
    pub source: Box<dyn Source3D>,
}
impl RotatePoint {
    pub fn from_angles(x_angle_deg: f32, y_angle_deg: f32, z_angle_deg: f32, source: Box<dyn Source3D>) -> Self {
        let deg = (std::f32::consts::PI / 180.0) as f64;
        let cx = (x_angle_deg as f64 * deg).cos() as f32 as f64;
        let cy = (y_angle_deg as f64 * deg).cos() as f32 as f64;
        let cz = (z_angle_deg as f64 * deg).cos() as f32 as f64;
        let sx = (x_angle_deg as f64 * deg).sin() as f32 as f64;
        let sy = (y_angle_deg as f64 * deg).sin() as f32 as f64;
        let sz = (z_angle_deg as f64 * deg).sin() as f32 as f64;
        let m = [
            [(sy*sx*sz + cy*cz) as f32, (cx*sz) as f32, (sy*cz - cy*sx*sz) as f32],
            [(sy*sx*cz - cy*sz) as f32, (cx*cz) as f32, (-cy*sx*cz - sy*sz) as f32],
            [(-sy*cx) as f32, sx as f32, (cy*cx) as f32],
        ];
        Self { m, source }
    }
}
impl Source3D for RotatePoint {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        let dot = |row: [f32;3]| (row[0] as f64*x as f64 + row[1] as f64*y as f64 + row[2] as f64*z as f64) as f32;
        let x2 = dot(self.m[0]);
        let y2 = dot(self.m[1]);
        let z2 = dot(self.m[2]);
        self.source.get_value_3(x2, y2, z2)
    }
}

/// TranslatePoint — add offsets to input before sampling.
pub struct TranslatePoint {
    pub dx: f32, pub dy: f32, pub dz: f32,
    pub source: Box<dyn Source3D>,
}

/// ONI's Displace transformer supplies X and Z noise; Y is a zero constant.
pub struct Displace {
    pub source: Box<dyn Source3D>,
    pub x_displace: Box<dyn Source3D>,
    pub z_displace: Box<dyn Source3D>,
}
impl Source3D for Displace {
    fn get_value_3(&self,x:f32,y:f32,z:f32)->f32 {
        let dx=self.x_displace.get_value_3(x,y,z);
        let dz=self.z_displace.get_value_3(x,y,z);
        self.source.get_value_3(x+dx,y,z+dz)
    }
}

pub struct Turbulence {
    pub source: Box<dyn Source3D>,
    pub x_distort: Box<dyn Source3D>,
    pub z_distort: Box<dyn Source3D>,
    pub power: f32,
}
impl Source3D for Turbulence {
    fn get_value_3(&self,x:f32,y:f32,z:f32)->f32 {
        let dx=self.x_distort.get_value_3(x+0.1894226,y+0.9937134,z+0.47816467);
        let dz=self.z_distort.get_value_3(x+0.821228,y+0.1710968,z+0.6842804);
        self.source.get_value_3((x as f64+dx as f64*self.power as f64) as f32,y,
            (z as f64+dz as f64*self.power as f64) as f32)
    }
}
impl Source3D for TranslatePoint {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        self.source.get_value_3(x + self.dx, y + self.dy, z + self.dz)
    }
}

/// ScalePoint — multiply input by per-axis scales before sampling.
pub struct ScalePoint {
    pub sx: f32, pub sy: f32, pub sz: f32,
    pub source: Box<dyn Source3D>,
}
impl Source3D for ScalePoint {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        self.source.get_value_3(x * self.sx, y * self.sy, z * self.sz)
    }
}

/// Cerp-interpolated Curve modifier (matches `ControlPoint` list).
pub struct Curve {
    /// Sorted ascending by input.
    pub control_points: Vec<(f32, f32)>,
    pub source: Box<dyn Source3D>,
}
impl Source3D for Curve {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        let v = self.source.get_value_3(x, y, z);
        let n = self.control_points.len();
        if n == 0 { return v; }
        let mut i = 0;
        while i < n && !(v < self.control_points[i].0) { i += 1; }
        let idx0 = (i as i32 - 2).max(0).min(n as i32 - 1) as usize;
        let idx1 = (i as i32 - 1).max(0).min(n as i32 - 1) as usize;
        let idx2 = (i as i32).max(0).min(n as i32 - 1) as usize;
        let idx3 = (i as i32 + 1).max(0).min(n as i32 - 1) as usize;
        if idx1 == idx2 {
            return self.control_points[idx1].1;
        }
        let input1 = self.control_points[idx1].0;
        let input2 = self.control_points[idx2].0;
        let a = ((v as f64 - input1 as f64) / (input2 as f64 - input1 as f64)) as f32;
        cerp(
            self.control_points[idx0].1,
            self.control_points[idx1].1,
            self.control_points[idx2].1,
            self.control_points[idx3].1,
            a,
        )
    }
}

#[inline]
fn cerp(n0: f32, n1: f32, n2: f32, n3: f32, a: f32) -> f32 {
    let p = (n3 as f64 - n2 as f64 - (n0 as f64 - n1 as f64)) as f32;
    let q = (n0 as f64 - n1 as f64 - p as f64) as f32;
    let r = n2 - n0;
    let a = a as f64;
    (p as f64 * a * a * a + q as f64 * a * a + r as f64 * a + n1 as f64) as f32
}

// ---------------- Combiners (two-source) ----------------

pub struct Add { pub left: Box<dyn Source3D>, pub right: Box<dyn Source3D> }
impl Source3D for Add {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        self.left.get_value_3(x, y, z) + self.right.get_value_3(x, y, z)
    }
}
pub struct Max { pub left: Box<dyn Source3D>, pub right: Box<dyn Source3D> }
impl Source3D for Max {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        self.left.get_value_3(x, y, z).max(self.right.get_value_3(x, y, z))
    }
}
pub struct Min { pub left: Box<dyn Source3D>, pub right: Box<dyn Source3D> }
impl Source3D for Min {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        self.left.get_value_3(x, y, z).min(self.right.get_value_3(x, y, z))
    }
}
pub struct Multiply { pub left: Box<dyn Source3D>, pub right: Box<dyn Source3D> }
impl Source3D for Multiply {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        self.left.get_value_3(x, y, z) * self.right.get_value_3(x, y, z)
    }
}
pub struct Power { pub left: Box<dyn Source3D>, pub right: Box<dyn Source3D> }
impl Source3D for Power {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        (self.left.get_value_3(x, y, z) as f64).powf(self.right.get_value_3(x, y, z) as f64) as f32
    }
}

// ---------------- Selector ----------------

/// 3-input selector: picks `left` or `right` based on `control`'s value
/// relative to [lower, upper], with edge falloff blending.
pub struct Select {
    pub lower: f32,
    pub upper: f32,
    pub edge_falloff: f32,
    pub control: Box<dyn Source3D>,
    pub left: Box<dyn Source3D>,
    pub right: Box<dyn Source3D>,
}
impl Source3D for Select {
    fn get_value_3(&self, x: f32, y: f32, z: f32) -> f32 {
        let v = self.control.get_value_3(x, y, z);
        if (self.edge_falloff as f64) > 0.0 {
            if (v as f64) < self.lower as f64 - self.edge_falloff as f64 {
                return self.left.get_value_3(x, y, z);
            }
            if (v as f64) < self.lower as f64 + self.edge_falloff as f64 {
                let a1 = self.lower - self.edge_falloff;
                let a2 = self.lower + self.edge_falloff;
                let a = s_curve3(((v as f64 - a1 as f64) / (a2 as f64 - a1 as f64)) as f32);
                return lerp(self.left.get_value_3(x, y, z), self.right.get_value_3(x, y, z), a);
            }
            if (v as f64) < self.upper as f64 - self.edge_falloff as f64 {
                return self.right.get_value_3(x, y, z);
            }
            if (v as f64) < self.upper as f64 + self.edge_falloff as f64 {
                let a1 = self.upper - self.edge_falloff;
                let a2 = self.upper + self.edge_falloff;
                let a = s_curve3(((v as f64 - a1 as f64) / (a2 as f64 - a1 as f64)) as f32);
                return lerp(self.right.get_value_3(x, y, z), self.left.get_value_3(x, y, z), a);
            }
            return self.left.get_value_3(x, y, z);
        }
        if v < self.lower || v > self.upper {
            self.left.get_value_3(x, y, z)
        } else {
            self.right.get_value_3(x, y, z)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voronoi_near_tie_matches_aridio_game_noise() {
        // V-HTFST-C-1176155540, cell (102,119), jungle regional sample
        // after Scale2d. The game gives normalized base noise 0.77137327
        // and Carbon here. Per-operation f32 rounding chooses the other
        // lattice point (-0.3320712), yielding noise 0.33239785 and Hydrogen.
        let vor = VoronoiFilter {
            frequency: 32.0, displacement: 1.0, distance: false,
            source: Box::new(BevinsValue { seed: 1176155545, quality: NoiseQuality::Standard }),
            lattice: Default::default(),
        };
        let actual = vor.get_value(f32::from_bits(0x3f1cac0e), 0.0, f32::from_bits(0x3ef3b64b));
        assert_eq!(actual.to_bits(), 0.539808988571167_f32.to_bits());
    }

    // These historical CoreCLR filter/transformer samples are only numerical
    // smoke checks. Exact composed-tree parity is checked against live probes.
    fn assert_coreclr_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() <= 1e-6, "actual={actual} expected={expected}");
    }

    #[test]
    fn ridged_multi_fractal_bit_exact() {
        let mk = || Box::new(ImprovedPerlin::new(1, NoiseQuality::Best)) as Box<dyn Source3D>;
        let rmf = RidgedMultiFractal {
            frequency: 10.0, lacunarity: 3.0, octave_count: 10.0,
            offset: 1.0, gain: 2.0,
            spectral: SpectralWeights::new(3.0, 0.9),
            source: mk(),
        };
        assert_eq!(rmf.get_value(0.001, 0.002, 0.003), 1.469_993_4);
        assert_eq!(rmf.get_value(0.5, 0.5, 0.0), 1.592_379_9);
    }

    #[test]
    fn billow_hybrid_sin_coreclr_smoke() {
        let mk = || Box::new(ImprovedPerlin::new(1, NoiseQuality::Best)) as Box<dyn Source3D>;
        let billow = Billow {
            frequency: 4.0, lacunarity: 2.0, octave_count: 6.0,
            scale: 1.0, bias: 0.0,
            spectral: SpectralWeights::new(2.0, 0.9),
            source: mk(),
        };
        assert_coreclr_close(billow.get_value(0.1, 0.2, 0.3), 0.229_903_2);

        let hmf = HybridMultiFractal {
            frequency: 4.0, lacunarity: 2.0, octave_count: 6.0,
            offset: 0.7, gain: 1.0,
            spectral: SpectralWeights::new(2.0, 0.25),
            source: mk(),
        };
        assert_coreclr_close(hmf.get_value(0.1, 0.2, 0.3), 1.219_790_7);

        let sin = SinFractal {
            frequency: 4.0, lacunarity: 2.0, octave_count: 6.0,
            spectral: SpectralWeights::new(2.0, 0.9),
            source: mk(),
        };
        assert_coreclr_close(sin.get_value(0.1, 0.2, 0.3), 0.323_951_45);
    }

    #[test]
    fn transformers_coreclr_smoke() {
        let inner = || Box::new(SimplexPerlin::new(1)) as Box<dyn Source3D>;
        let rot = RotatePoint::from_angles(30.0, 45.0, 60.0, inner());
        assert_coreclr_close(rot.get_value_3(0.1, 0.2, 0.3), 0.047_880_52);
        let tra = TranslatePoint { dx: 1.0, dy: 2.0, dz: 3.0, source: inner() };
        assert_coreclr_close(tra.get_value_3(0.1, 0.2, 0.3), 0.082_627_91);
        let scp = ScalePoint { sx: 2.0, sy: 3.0, sz: 4.0, source: inner() };
        assert_coreclr_close(scp.get_value_3(0.1, 0.2, 0.3), 0.870_681_17);
    }

    #[test]
    fn voronoi_bit_exact() {
        let mk = || Box::new(ImprovedPerlin::new(1, NoiseQuality::Best)) as Box<dyn Source3D>;
        let vor = VoronoiFilter { frequency: 4.0, displacement: 1.0, distance: false, source: mk(), lattice: Default::default() };
        assert_eq!(vor.get_value(0.1, 0.2, 0.3), 0.0);
        let vord = VoronoiFilter { frequency: 4.0, displacement: 1.0, distance: true, source: mk(), lattice: Default::default() };
        assert_eq!(vord.get_value(0.1, 0.2, 0.3), -0.151_471_85);
    }

    #[test]
    fn simplex_perlin_bit_exact_2d() {
        let cases = [
            (42i32, -0.734_293_76_f32),
            (1, 0.112_907_1),
            (100, -0.120_512_82),
            (1_000_172_956, 0.168_665_95),
        ];
        for (seed, expected) in cases {
            let p = SimplexPerlin::new(seed);
            assert_eq!(p.get_value_2(0.25, 0.5), expected, "seed={seed}");
        }
    }
}
