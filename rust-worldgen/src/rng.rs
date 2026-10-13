//! Port of KRandom (ONI's seeded RNG, a copy of .NET's System.Random).
//! Byte-exact with the C# reference implementation.

const MBIG: i32 = i32::MAX;
const MSEED: i32 = 161_803_398;

pub struct KRandom {
    seed_array: [i32; 56],
    inext: usize,
    inextp: usize,
}

#[derive(Debug,Clone,serde::Serialize,serde::Deserialize,PartialEq)]
pub struct RandomState {pub seed_array:Vec<i32>,pub inext:usize,pub inextp:usize}

impl KRandom {
    pub fn state(&self)->RandomState {RandomState {seed_array:self.seed_array.to_vec(),inext:self.inext,inextp:self.inextp}}
    pub fn from_state(state:&RandomState)->Result<Self,String> {
        if state.seed_array.len()!=56||state.inext>55||state.inextp>55||state.seed_array.iter().any(|&v|v<0||v==i32::MAX) {return Err("invalid KRandom state".into());}
        Ok(Self {seed_array:state.seed_array.clone().try_into().unwrap(),inext:state.inext,inextp:state.inextp})
    }
    pub fn new(seed: i32) -> Self {
        let abs_seed = if seed == i32::MIN {
            i32::MAX
        } else {
            seed.wrapping_abs()
        };
        let mut num: i32 = MSEED.wrapping_sub(abs_seed);
        let mut seed_array = [0i32; 56];
        seed_array[55] = num;
        let mut num2: i32 = 1;
        for i in 1..55 {
            let idx = (21 * i) % 55;
            seed_array[idx] = num2;
            num2 = num.wrapping_sub(num2);
            if num2 < 0 {
                num2 = num2.wrapping_add(MBIG);
            }
            num = seed_array[idx];
        }
        for _ in 1..5 {
            for k in 1..56 {
                seed_array[k] = seed_array[k].wrapping_sub(seed_array[1 + (k + 30) % 55]);
                if seed_array[k] < 0 {
                    seed_array[k] = seed_array[k].wrapping_add(MBIG);
                }
            }
        }
        Self { seed_array, inext: 0, inextp: 21 }
    }

    fn internal_sample(&mut self) -> i32 {
        let mut num = self.inext;
        let mut num2 = self.inextp;
        num += 1;
        if num >= 56 {
            num = 1;
        }
        num2 += 1;
        if num2 >= 56 {
            num2 = 1;
        }
        let mut num3 = self.seed_array[num].wrapping_sub(self.seed_array[num2]);
        if num3 == i32::MAX {
            num3 -= 1;
        }
        if num3 < 0 {
            num3 = num3.wrapping_add(MBIG);
        }
        self.seed_array[num] = num3;
        self.inext = num;
        self.inextp = num2;
        num3
    }

    fn sample(&mut self) -> f64 {
        // Matches C#: InternalSample() * (1.0 / int.MaxValue) literal-equivalent
        self.internal_sample() as f64 * 4.656_612_875_245_797E-10
    }

    fn sample_large_range(&mut self) -> f64 {
        let mut num = self.internal_sample();
        if self.internal_sample() % 2 == 0 {
            num = -num;
        }
        (num as f64 + 2_147_483_646.0) / 4_294_967_293.0
    }

    pub fn next_raw(&mut self) -> i32 {
        self.internal_sample()
    }

    pub fn next_max(&mut self, max_value: i32) -> i32 {
        (self.sample() * max_value as f64) as i32
    }

    pub fn next_range(&mut self, min_value: i32, max_value: i32) -> i32 {
        let range = (max_value as i64) - (min_value as i64);
        if range <= MBIG as i64 {
            (self.sample() * range as f64) as i32 + min_value
        } else {
            ((self.sample_large_range() * range as f64) as i64 + min_value as i64) as i32
        }
    }

    pub fn next_double(&mut self) -> f64 {
        self.sample()
    }

    pub fn next_bytes(&mut self, buf: &mut [u8]) {
        for b in buf.iter_mut() {
            *b = (self.internal_sample() % 256) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values from a standalone dotnet run of KRandom.
    // Ten pulls of Next() for each seed.

    fn first_n(seed: i32, n: usize) -> Vec<i32> {
        let mut r = KRandom::new(seed);
        (0..n).map(|_| r.next_raw()).collect()
    }

    #[test]
    fn seed_42() {
        let expected = [1434747710i32, 302596119, 269548474, 1122627734, 361709742, 563913476, 1555655117, 1101493307, 372913049, 1634773126];
        assert_eq!(&first_n(42, 10)[..], &expected);
    }

    #[test]
    fn seed_1() {
        let expected = [534011718i32, 237820880, 1002897798, 1657007234, 1412011072, 929393559, 760389092, 2026928803, 217468053, 1379662799];
        assert_eq!(&first_n(1, 10)[..], &expected);
    }

    #[test]
    fn seed_123456() {
        let expected = [570869451i32, 228016717, 63082525, 791561502, 1781384151, 1044764965, 415934392, 841409416, 1173634327, 1737775931];
        assert_eq!(&first_n(123456, 10)[..], &expected);
    }

    #[test]
    fn seed_1000172956() {
        let expected = [1090958373i32, 1970979990, 885807301, 814130090, 298692068, 237326410, 1264128120, 1255570224, 2012729699, 1354596832];
        assert_eq!(&first_n(1000172956, 10)[..], &expected);
    }

    #[test]
    fn seed_negative_one() {
        // Math.Abs(-1) == 1, so seed=-1 and seed=1 produce identical streams.
        let expected = [534011718i32, 237820880, 1002897798, 1657007234, 1412011072, 929393559, 760389092, 2026928803, 217468053, 1379662799];
        assert_eq!(&first_n(-1, 10)[..], &expected);
    }
}
