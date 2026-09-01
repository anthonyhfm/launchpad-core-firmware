// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use super::hw;

const NCOLS: usize = 3;
const NLEDBITS: usize = 56;
const NLED: usize = NCOLS * NLEDBITS;
const NSW: usize = 80;
const NSWBITS: usize = 96;
const NSWBYTES: usize = 4;
const LED_BYTES_PER_COL: usize = 7;
const SHIFT_BYTES: usize = LED_BYTES_PER_COL;
const EVENT_QUEUE_LEN: usize = 8;
const ENABLE_SWITCH_SCAN: bool = true;

const BLANK_TICKS: u16 = 22;
const SHIFT_TICKS: u16 = 15;

const BRIGHT_TICKS: [u16; 6] = [2, 90, 6, 34, 10, 18];
const BIT_ORDER: [usize; 6] = [0, 5, 1, 4, 2, 3];
const COL_ORDER: [u8; 9] = [0, 1, 2, 1, 2, 0, 2, 0, 1];

const SWITCH_MAP: [u8; NSWBITS] = [
    0x06, 0x0f, 0x18, 0x21, 0x2a, 0x33, 0x3c, 0x45, 0x03, 0x0c, 0x15, 0x1e, 0x27, 0x30, 0x39, 0x42,
    0x00, 0x09, 0x12, 0x1b, 0x24, 0x2d, 0x36, 0x3f, 0x48, 0x4b, 0x4e, 0x50, 0x50, 0x50, 0x50, 0x50,
    0x07, 0x10, 0x19, 0x22, 0x2b, 0x34, 0x3d, 0x46, 0x04, 0x0d, 0x16, 0x1f, 0x28, 0x31, 0x3a, 0x43,
    0x01, 0x0a, 0x13, 0x1c, 0x25, 0x2e, 0x37, 0x40, 0x49, 0x4c, 0x4f, 0x50, 0x50, 0x50, 0x50, 0x50,
    0x08, 0x11, 0x1a, 0x23, 0x2c, 0x35, 0x3e, 0x47, 0x05, 0x0e, 0x17, 0x20, 0x29, 0x32, 0x3b, 0x44,
    0x02, 0x0b, 0x14, 0x1d, 0x26, 0x2f, 0x38, 0x41, 0x4a, 0x4d, 0x50, 0x50, 0x50, 0x50, 0x50, 0x50,
];

const GRN_MAP: [u8; 81] = [
    0x1f, 0x57, 0x8f, 0x27, 0x5f, 0x97, 0x2f, 0x67, 0x9f, 0x1e, 0x56, 0x8e, 0x26, 0x5e, 0x96, 0x2e,
    0x66, 0x9e, 0x1d, 0x55, 0x8d, 0x25, 0x5d, 0x95, 0x2d, 0x65, 0x9d, 0x1c, 0x54, 0x8c, 0x24, 0x5c,
    0x94, 0x2c, 0x64, 0x9c, 0x17, 0x4f, 0x87, 0x0f, 0x47, 0x7f, 0x07, 0x3f, 0x77, 0x16, 0x4e, 0x86,
    0x0e, 0x46, 0x7e, 0x06, 0x3e, 0x76, 0x15, 0x4d, 0x85, 0x0d, 0x45, 0x7d, 0x05, 0x3d, 0x75, 0x14,
    0x4c, 0x84, 0x0c, 0x44, 0x7c, 0x04, 0x3c, 0x74, 0x37, 0x6f, 0xa7, 0x36, 0x6e, 0xa6, 0x35, 0x6d,
    0x30,
];

const RED_MAP: [u8; 81] = [
    0x1b, 0x53, 0x8b, 0x23, 0x5b, 0x93, 0x2b, 0x63, 0x9b, 0x1a, 0x52, 0x8a, 0x22, 0x5a, 0x92, 0x2a,
    0x62, 0x9a, 0x19, 0x51, 0x89, 0x21, 0x59, 0x91, 0x29, 0x61, 0x99, 0x18, 0x50, 0x88, 0x20, 0x58,
    0x90, 0x28, 0x60, 0x98, 0x13, 0x4b, 0x83, 0x0b, 0x43, 0x7b, 0x03, 0x3b, 0x73, 0x12, 0x4a, 0x82,
    0x0a, 0x42, 0x7a, 0x02, 0x3a, 0x72, 0x11, 0x49, 0x81, 0x09, 0x41, 0x79, 0x01, 0x39, 0x71, 0x10,
    0x48, 0x80, 0x08, 0x40, 0x78, 0x00, 0x38, 0x70, 0x34, 0x6c, 0xa4, 0x33, 0x6b, 0xa3, 0x32, 0x6a,
    0x30,
];

fn apply_led_brightness(value: u8, brightness: u8) -> u8 {
    let value = value.min(0x3f);
    if value == 0 {
        return 0;
    }

    let x = brightness.min(7) as u32;
    ((((value as u32 - 1) * (63 * (x + 5) * (x + 5) - 144)) / (62 * 144)) + 1) as u8
}

static SURFACE: AtomicPtr<Surface> = AtomicPtr::new(ptr::null_mut());
static ISR_READY: AtomicBool = AtomicBool::new(false);

pub struct Surface {
    leds: [u8; NLED],
    raw_leds: [u8; NLED],
    switches: [u8; NSW],
    press_count: [u8; NSW],
    release_count: [u8; NSW],
    switch_scan: [u8; NCOLS * NSWBYTES],
    events: [SurfaceEvent; EVENT_QUEUE_LEN],
    event_head: u8,
    event_tail: u8,
    tx: [u8; SHIFT_BYTES],
    rx: [u8; SHIFT_BYTES],
    col: u8,
    col_group: u8,
    col_order: u8,
    bit: u8,
    phase: u8,
    brightness: u8,
}

impl Surface {
    pub const fn new() -> Self {
        Self {
            leds: [0; NLED],
            raw_leds: [0; NLED],
            switches: [0; NSW],
            press_count: [0; NSW],
            release_count: [0; NSW],
            switch_scan: [0; NCOLS * NSWBYTES],
            events: [SurfaceEvent::Release { index: 0 }; EVENT_QUEUE_LEN],
            event_head: 0,
            event_tail: 0,
            tx: [0xff; SHIFT_BYTES],
            rx: [0xff; SHIFT_BYTES],
            col: 0,
            col_group: 0,
            col_order: 0,
            bit: 0,
            phase: 0,
            brightness: 7,
        }
    }

    pub fn init(&mut self) {
        init_pins_and_spi();
        self.blank();
        self.deselect_cols();
    }

    pub fn start_scan(&mut self) {
        SURFACE.store(self as *mut Surface, Ordering::Release);
        ISR_READY.store(true, Ordering::Release);

        hw::pac::RCC.apb1enr().modify(|w| w.set_tim2en(true));
        hw::pac::TIM2.cr1().write(|w| w.set_cen(false));
        hw::pac::TIM2.psc().write(|w| *w = 24 - 1);
        hw::pac::TIM2.arr().write(|w| *w = hw::pac::timer::regs::ArrCore(BLANK_TICKS as u32));
        hw::pac::TIM2.cnt().write(|w| *w = hw::pac::timer::regs::CntCore(0));
        hw::pac::TIM2.egr().write(|w| w.set_ug(true));
        hw::pac::TIM2.sr().write(|w| w.set_uif(false));
        hw::pac::TIM2.dier().write(|w| w.set_uie(true));

        unsafe {
            cortex_m::peripheral::NVIC::unmask(hw::Interrupt::TIM2);
        }
        hw::pac::TIM2.cr1().modify(|w| w.set_cen(true));
    }

    pub fn set_rgb_led(&mut self, index: u8, r: u8, g: u8, _b: u8) {
        let key = index_to_key(index);
        if key >= RED_MAP.len() {
            return;
        }

        cortex_m::interrupt::free(|_| {
            self.set_key_rgb(key, r, g);
        });
    }

    pub fn fill(&mut self, rgb: u32) {
        let r = ((rgb >> 18) & 0x3f) as u8;
        let g = ((rgb >> 10) & 0x3f) as u8;
        for index in 0..100 {
            self.set_rgb_led(index as u8, r, g, 0);
        }
    }

    pub const fn brightness(&self) -> u8 {
        self.brightness
    }

    pub fn set_brightness(&mut self, brightness: u8) {
        cortex_m::interrupt::free(|_| {
            self.brightness = brightness.min(7);
            self.rebuild_scaled_leds();
        });
    }

    pub fn poll_event(&mut self) -> Option<SurfaceEvent> {
        if self.event_tail == self.event_head {
            return None;
        }

        let event = self.events[self.event_tail as usize];
        self.event_tail = (self.event_tail + 1) % EVENT_QUEUE_LEN as u8;
        Some(event)
    }

    pub fn tick_1khz(&mut self) {
        if !ENABLE_SWITCH_SCAN {
            return;
        }

        self.debounce_switches();
    }

    fn isr_step(&mut self) -> u16 {
        match self.phase {
            0 => {
                self.blank();
                self.prepare_shift_payload();
                self.phase = 1;
                SHIFT_TICKS
            }
            1 => {
                spi_transfer(&self.tx, &mut self.rx);
                self.phase = 2;
                SHIFT_TICKS
            }
            2 => {
                self.select_col(self.col);
                let ticks = BRIGHT_TICKS[self.bit as usize];
                self.phase = 3;
                ticks
            }
            3 => {
                hw::pac::GPIOA.bsrr().write(|w| w.set_bs(4, true));
                self.phase = 4;
                SHIFT_TICKS
            }
            _ => {
                if self.bit == 1 || self.bit == 3 || self.bit == 5 {
                    let tx = [0xff; NSWBYTES];
                    let base = self.col as usize * NSWBYTES;
                    spi1_transfer(
                        &tx,
                        (&mut self.switch_scan[base..base + NSWBYTES])
                            .try_into()
                            .unwrap(),
                    );
                }
                self.advance_scan();
                self.phase = 0;
                SHIFT_TICKS
            }
        }
    }

    fn prepare_shift_payload(&mut self) {
        self.tx.fill(0xff);
        let base = self.col as usize * NLEDBITS;
        let mask = 1u8 << BIT_ORDER[self.bit as usize];
        for byte in 0..LED_BYTES_PER_COL {
            let mut bits = 0u8;
            for lane in 0..8 {
                if self.leds[base + byte * 8 + lane] & mask != 0 {
                    bits |= 1 << lane;
                }
            }
            self.tx[byte] = !bits;
        }
    }

    fn advance_scan(&mut self) {
        self.col_order += 1;
        if self.col_order >= NCOLS as u8 {
            self.col_order = 0;
            self.bit += 1;
            if self.bit >= 6 {
                self.bit = 0;
                self.col_group += 1;
                if self.col_group >= NCOLS as u8 {
                    self.col_group = 0;
                }
            }
        }
        let order_index = self.col_group as usize * NCOLS + self.col_order as usize;
        self.col = COL_ORDER[order_index];
    }

    fn blank(&self) {
        hw::pac::GPIOA.bsrr().write(|w| w.set_br(4, true));
        self.deselect_cols();
    }

    fn deselect_cols(&self) {
        hw::pac::GPIOB.bsrr().write(|w| {
            w.set_bs(0, true);
            w.set_bs(1, true);
            w.set_bs(2, true);
        });
    }

    fn select_col(&self, col: u8) {
        self.deselect_cols();
        match col {
            0 => hw::pac::GPIOB.bsrr().write(|w| w.set_br(0, true)),
            1 => hw::pac::GPIOB.bsrr().write(|w| w.set_br(1, true)),
            _ => hw::pac::GPIOB.bsrr().write(|w| w.set_br(2, true)),
        }
    }

    fn debounce_switches(&mut self) {
        for bit in 0..NSWBITS {
            let key = SWITCH_MAP[bit] as usize;
            if key >= NSW {
                continue;
            }

            let byte = self.switch_scan[bit / 8];
            let pressed = byte & (0x80 >> (bit % 8)) != 0;
            if pressed {
                self.release_count[key] = 0;
                if self.switches[key] == 0 {
                    self.press_count[key] = self.press_count[key].saturating_add(1);
                    if self.press_count[key] >= 3 {
                        self.switches[key] = 1;
                        self.press_count[key] = 0;
                        self.push_event(SurfaceEvent::Press {
                            index: key_to_index(key),
                            value: 127,
                        });
                    }
                }
            } else {
                self.press_count[key] = 0;
                if self.switches[key] != 0 {
                    self.release_count[key] = self.release_count[key].saturating_add(1);
                    if self.release_count[key] >= 40 {
                        self.switches[key] = 0;
                        self.release_count[key] = 0;
                        self.push_event(SurfaceEvent::Release {
                            index: key_to_index(key),
                        });
                    }
                }
            }
        }
    }

    fn push_event(&mut self, event: SurfaceEvent) {
        let next = (self.event_head + 1) % EVENT_QUEUE_LEN as u8;
        if next == self.event_tail {
            return;
        }

        self.events[self.event_head as usize] = event;
        self.event_head = next;
    }

    fn set_key_rgb(&mut self, key: usize, r: u8, g: u8) {
        let r_idx = RED_MAP[key] as usize;
        let g_idx = GRN_MAP[key] as usize;
        self.raw_leds[r_idx] = r.min(0x3f);
        self.raw_leds[g_idx] = g.min(0x3f);
        self.leds[r_idx] = apply_led_brightness(r, self.brightness);
        self.leds[g_idx] = apply_led_brightness(g, self.brightness);
    }

    fn rebuild_scaled_leds(&mut self) {
        for index in 0..NLED {
            self.leds[index] = apply_led_brightness(self.raw_leds[index], self.brightness);
        }
    }
}

#[derive(Copy, Clone)]
pub enum SurfaceEvent {
    Press { index: u8, value: u8 },
    Release { index: u8 },
}

fn key_to_index(key: usize) -> u8 {
    if key < 72 {
        let row = key / 9;
        let col = key % 9;
        let mirrored_row = 7 - row;
        ((mirrored_row + 1) * 10 + (col + 1)) as u8
    } else {
        91 + (key - 72) as u8
    }
}

fn index_to_key(index: u8) -> usize {
    match index {
        11..=89 if index % 10 != 0 => (8 - index as usize / 10) * 9 + (index as usize % 10 - 1),
        91..=98 => 72 + (index - 91) as usize,
        99 => 80,
        _ => RED_MAP.len(),
    }
}

fn init_pins_and_spi() {
    hw::init_gpio_clocks();
    hw::pac::RCC.apb2enr().modify(|w| w.set_spi1en(true));
    hw::pac::RCC.apb1enr().modify(|w| w.set_spi2en(true));

    hw::pac::SPI1.cr1().write(|w| {
        w.set_cpol(hw::pac::spi::vals::Cpol::IDLE_LOW);
        w.set_mstr(hw::pac::spi::vals::Mstr::MASTER);
        w.set_br(hw::pac::spi::vals::Br::DIV4);
        w.set_lsbfirst(hw::pac::spi::vals::Lsbfirst::LSBFIRST);
        w.set_ssm(true);
        w.set_ssi(true);
        w.set_spe(true);
    });

    hw::pac::SPI2.cr1().write(|w| {
        w.set_cpha(hw::pac::spi::vals::Cpha::SECOND_EDGE);
        w.set_mstr(hw::pac::spi::vals::Mstr::MASTER);
        w.set_br(hw::pac::spi::vals::Br::DIV4);
        w.set_lsbfirst(hw::pac::spi::vals::Lsbfirst::LSBFIRST);
        w.set_ssm(true);
        w.set_ssi(true);
        w.set_spe(true);
    });
}

fn spi1_transfer(tx: &[u8; NSWBYTES], rx: &mut [u8; NSWBYTES]) {
    for (index, &byte) in tx.iter().enumerate() {
        while !hw::pac::SPI1.sr().read().txe() {}
        unsafe {
            ptr::write_volatile(hw::pac::SPI1.dr().as_ptr() as *mut u8, byte);
        }
        while !hw::pac::SPI1.sr().read().rxne() {}
        rx[index] = unsafe { ptr::read_volatile(hw::pac::SPI1.dr().as_ptr() as *const u8) };
    }

    while hw::pac::SPI1.sr().read().bsy() {}
}

fn spi_transfer(tx: &[u8; SHIFT_BYTES], rx: &mut [u8; SHIFT_BYTES]) {
    for (index, &byte) in tx.iter().enumerate() {
        while !hw::pac::SPI2.sr().read().txe() {}
        unsafe {
            ptr::write_volatile(hw::pac::SPI2.dr().as_ptr() as *mut u8, byte);
        }
        while !hw::pac::SPI2.sr().read().rxne() {}
        rx[index] = unsafe { ptr::read_volatile(hw::pac::SPI2.dr().as_ptr() as *const u8) };
    }

    while hw::pac::SPI2.sr().read().bsy() {}
}

#[unsafe(export_name = "TIM2")]
pub extern "C" fn tim2_handler() {
    if !hw::pac::TIM2.sr().read().uif() {
        return;
    }

    let ticks = if ISR_READY.load(Ordering::Acquire) {
        let ptr = SURFACE.load(Ordering::Acquire);
        if ptr.is_null() {
            BLANK_TICKS
        } else {
            unsafe { (&mut *ptr).isr_step() }
        }
    } else {
        BLANK_TICKS
    };

    let ticks = ticks.max(2) as u32;
    hw::pac::TIM2.arr().write(|w| *w = hw::pac::timer::regs::ArrCore(ticks));
    hw::pac::TIM2.cnt().write(|w| *w = hw::pac::timer::regs::CntCore(0));
    hw::pac::TIM2.sr().write(|w| w.set_uif(false));
}
