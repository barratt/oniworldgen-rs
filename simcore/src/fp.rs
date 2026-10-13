//! Floating-point primitives with the exact semantics of the SSE
//! instructions the native code uses. Rust's `f32::min`/`max` and `as`
//! casts differ from `minss`/`maxss`/`cvttss2si` on NaN, equality and
//! overflow, so every such site goes through these helpers.

/// `minss a, b`: `a < b ? a : b` (returns `b` on NaN or equality).
#[inline(always)]
pub fn minss(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}
/// `maxss a, b`: `a > b ? a : b`.
#[inline(always)]
pub fn maxss(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}
#[inline(always)]
pub fn minsd(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}
#[inline(always)]
pub fn maxsd(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}
/// `cvttss2si r32`: truncation; NaN and out-of-range give i32::MIN.
#[inline(always)]
pub fn cvttss2si(x: f32) -> i32 {
    if x.is_nan() || x >= 2147483648.0 || x < -2147483648.0 { i32::MIN } else { x as i32 }
}
/// Flush a denormal result to signed zero (MXCSR.FTZ, used on the sim
/// thread; the temperature worker thread runs without it).
#[inline(always)]
pub fn ftz(x: f32) -> f32 {
    if x != 0.0 && x.abs() < f32::MIN_POSITIVE { 0.0f32.copysign(x) } else { x }
}

/// `logf` as reached from the temperature exchange (CRT, FMA3 path).
/// Computed in double precision and rounded once.
pub fn logf(x: f32) -> f32 {
    libm::log(x as f64) as f32
}
/// `expf` as reached from the temperature exchange.
pub fn expf(x: f32) -> f32 {
    libm::exp(x as f64) as f32
}
/// `powf` as reached from germ growth (CRT, FMA3 path), which only ever
/// raises 2. The CRT result is not correctly rounded: measured against the
/// DLL, its error is that of the textbook scheme 2^y = 2^(k/64) * p(r), with
/// k = round(64y), r = y - k/64 and p the cubic Taylor polynomial of 2^r
/// (a relative error of about (r ln 2)^4 / 24, below the true value). This
/// evaluates that scheme in double precision and rounds once, which also
/// reproduces where the CRT rounds the other way near float midpoints.
/// Other bases fall back to a double-precision pow.
pub fn powf(x: f32, y: f32) -> f32 {
    if x == 2.0 && y.is_finite() && y.abs() < 126.0 {
        return exp2_cubic(y as f64) as f32;
    }
    libm::pow(x as f64, y as f64) as f32
}

fn exp2_cubic(y: f64) -> f64 {
    let k = libm::round(y * 64.0);
    let t = (y - k / 64.0) * core::f64::consts::LN_2;
    let p = 1.0 + t * (1.0 + t * (0.5 + t * (1.0 / 6.0)));
    let k = k as i32;
    libm::scalbn(libm::exp2(k.rem_euclid(64) as f64 / 64.0) * p, k.div_euclid(64))
}

/// An f32 computed on the native sim thread, whose MXCSR has FTZ set: every
/// arithmetic result that is denormal becomes signed zero. Comparisons and
/// moves are plain. Use for all sim-thread phases (not temperature).
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Default)]
pub struct F(pub f32);

impl F {
    #[inline(always)]
    pub fn v(self) -> f32 {
        self.0
    }
    #[inline(always)]
    pub fn abs(self) -> F {
        F(self.0.abs())
    }
    #[inline(always)]
    pub fn from_i32(i: i32) -> F {
        F(i as f32)
    }
}
macro_rules! ftz_op {
    ($tr:ident, $f:ident, $op:tt) => {
        impl std::ops::$tr for F {
            type Output = F;
            #[inline(always)]
            fn $f(self, o: F) -> F { F(ftz(self.0 $op o.0)) }
        }
        impl std::ops::$tr<f32> for F {
            type Output = F;
            #[inline(always)]
            fn $f(self, o: f32) -> F { F(ftz(self.0 $op o)) }
        }
    };
}
ftz_op!(Add, add, +);
ftz_op!(Sub, sub, -);
ftz_op!(Mul, mul, *);
ftz_op!(Div, div, /);
impl std::ops::Neg for F {
    type Output = F;
    #[inline(always)]
    fn neg(self) -> F {
        F(-self.0)
    }
}
impl PartialEq<f32> for F {
    fn eq(&self, o: &f32) -> bool {
        self.0 == *o
    }
}
impl PartialOrd<f32> for F {
    fn partial_cmp(&self, o: &f32) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(o)
    }
}
#[inline(always)]
pub fn fmin(a: F, b: F) -> F {
    F(minss(a.0, b.0))
}
#[inline(always)]
pub fn fmax(a: F, b: F) -> F {
    F(maxss(a.0, b.0))
}
