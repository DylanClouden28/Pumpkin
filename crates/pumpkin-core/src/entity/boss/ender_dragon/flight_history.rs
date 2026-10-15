#[derive(Clone, Copy, Default, Debug)]
pub struct Sample {
    pub y: f64,
    pub y_rot: f32,
}

const LENGTH: usize = 64;
const MASK: i32 = 63;

#[derive(Clone, Copy, Debug)]
pub struct DragonFlightHistory {
    samples: [Sample; LENGTH],
    head: i32,
}

impl Default for DragonFlightHistory {
    fn default() -> Self {
        Self {
            samples: [Sample::default(); LENGTH],
            head: -1,
        }
    }
}

impl DragonFlightHistory {
    pub const fn record(&mut self, y: f64, y_rot: f32) {
        let sample = Sample { y, y_rot };
        if self.head < 0 {
            // The first sample fills the whole history, so early lookups don't read y = 0.
            self.samples = [sample; LENGTH];
        }
        self.head += 1;
        if self.head >= LENGTH as i32 {
            self.head = 0;
        }
        self.samples[self.head as usize] = sample;
    }

    #[must_use]
    pub const fn get(&self, offset: i32) -> Sample {
        self.samples[((self.head - offset) & MASK) as usize]
    }
}
