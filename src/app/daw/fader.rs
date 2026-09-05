// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister
pub const UNIPOLAR: [u8; 8] = [0, 17, 34, 52, 70, 89, 108, 127];
pub const BIPOLAR: [u8; 8] = [0, 21, 42, 63, 64, 85, 106, 127];

#[derive(Clone, Copy)]
pub struct Fader {
    pub bipolar: bool,
    pub cc: u8,
    pub colour: u8,
    pub value: u8,
    start: u16,
    target: u8,
    elapsed: u16,
    duration: u16,
}

impl Fader {
    pub const fn new() -> Self {
        Self {
            bipolar: false,
            cc: 0,
            colour: 0,
            value: 0,
            start: 0,
            target: 0,
            elapsed: 0,
            duration: 0,
        }
    }

    pub fn moving(&self) -> bool {
        self.duration != 0
    }

    pub fn stop(&mut self) {
        self.duration = 0;
    }

    pub fn press(&mut self, step: usize, velocity: u8) -> bool {
        if self.colour == 0 {
            return false;
        }

        let values = if self.bipolar { BIPOLAR } else { UNIPOLAR };
        self.start = self.position();
        self.target = values[step];
        self.elapsed = 0;
        self.duration = (20 + (127 - velocity.clamp(1, 127)) as u32 * 1980 / 126) as u16;

        if self.start == (self.target as u16) << 8 {
            self.stop();
            return true;
        }

        false
    }

    pub fn tick(&mut self) -> bool {
        if !self.moving() {
            return false;
        }

        self.elapsed = self.elapsed.saturating_add(1);
        if self.elapsed < self.duration && self.elapsed % 2 != 0 {
            return false;
        }

        let next = (self.position() >> 8) as u8;
        if self.elapsed >= self.duration {
            self.stop();
        }

        let changed = next != self.value;
        self.value = next;
        changed
    }

    fn position(&self) -> u16 {
        if !self.moving() {
            return (self.value as u16) << 8;
        }

        (self.start as i32
            + (((self.target as i32) << 8) - self.start as i32)
                * self.elapsed.min(self.duration) as i32
                / self.duration as i32) as u16
    }

    pub fn animation_frame(&self) -> bool {
        self.elapsed % 2 == 0 || !self.moving()
    }

    pub fn brightness(&self, step: usize) -> u8 {
        if self.colour == 0 {
            return 0;
        }

        let position = self.position() as i32;
        if !self.bipolar {
            let (low, high) = if step == 0 {
                (0, 1)
            } else {
                (UNIPOLAR[step - 1], UNIPOLAR[step])
            };
            return ((position - ((low as i32) << 8)) * 255 / (((high - low) as i32) << 8))
                .clamp(0, 255) as u8;
        }

        let at_stop = |value: u8| -> i32 {
            let lit = if (63..=64).contains(&value) {
                step == 3 || step == 4
            } else if value < 63 {
                step <= 3 && value <= BIPOLAR[step]
            } else {
                step >= 4 && value >= BIPOLAR[step]
            };
            if lit { 255 } else { 0 }
        };

        for pair in BIPOLAR.windows(2) {
            let low = (pair[0] as i32) << 8;
            let high = (pair[1] as i32) << 8;
            if position <= high {
                let a = at_stop(pair[0]);
                return (a + (at_stop(pair[1]) - a) * (position - low) / (high - low)) as u8;
            }
        }

        at_stop(127) as u8
    }
}

#[derive(Clone, Copy)]
pub struct Bank {
    pub horizontal: bool,
    pub faders: [Fader; 8],
}

impl Bank {
    pub const fn new() -> Self {
        Self {
            horizontal: false,
            faders: [Fader::new(); 8],
        }
    }

    pub fn stop(&mut self) {
        for f in &mut self.faders {
            f.stop();
        }
    }
}
