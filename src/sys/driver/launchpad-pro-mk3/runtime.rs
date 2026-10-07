// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use embassy_embedded_hal::SetConfig;
use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::mode::Blocking;
use embassy_stm32::peripherals;
use embassy_stm32::usart::{Config as UartConfig, Uart};
use embassy_stm32::Peri;
use embedded_hal_nb::nb;
use embedded_hal_nb::serial::Read;

use crate::sys::driver::common::storage::ExtFlash;
use super::input_filter::{
    M0InputFilter, PAD_COUNT as M0_PAD_COUNT, PAD_PRESS_THRESHOLD as M0_PAD_PRESS_THRESHOLD,
};
use super::led::LedSystem;
use super::map::{BUTTON_REMAP, BUTTON_REMAP_SIZE, LED_REMAP_SIZE};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use crate::app::SurfaceEvent;
use crate::sys::driver::Driver;

const M0_UART_BAUD_FIRMWARE: u32 = 0x005B_3EF5;
const M0_UART_BAUD_BOOTLOADER: u32 = 0x005A_CA30;
const M0_RETRY_INTERVAL_COLD_MS: u64 = 300;
const M0_RETRY_INTERVAL_RUNTIME_MS: u64 = 25;
const M0_HANDSHAKE_SETTLE_COLD_MS: u64 = 100;
const M0_HANDSHAKE_SETTLE_RETRY_MS: u64 = 5;
const M0_RESET_SETTLE_MS: u64 = 100;
const M0_BTN_FRAME_SIZE: usize = 140;
const M0_BTN_FRAME_DMA_SIZE: usize = 160;
const M0_BTN_FRAME_SYNC: u8 = 0xAA;
const M0_BTN_STREAM_ACK_VALUE: u16 = 0x0001;
const M0_STREAM_WARMUP_FRAMES: u8 = 2;
const M0_STREAM_MISS_TOLERANCE: u8 = 2;
const M0_LED_TX_WARMUP_SLOTS: u8 = 16;
const M0_LED_TX_WINDOW_SLOTS: u8 = 32;
const M0_BTN_STALL_RESTART_MS: u64 = 300;
const M0_BTN_RX_TIMEOUT_FAST_MS: u64 = 5;
const M0_BTN_RX_TIMEOUT_SLOW_MS: u64 = 20;
const M0_SIDE_BYTES: usize = 8;
const M0_SIDE_COUNT: usize = M0_SIDE_BYTES * 8;
const M0_BTN_DATA_OFFSET: usize = 4;
const M0_BTN_DIGITAL_OFFSET: usize = 0x84;
const M0_SIDE_AUX_BYTE_INDEX: usize = 1;
const M0_SIDE_SETUP_BIT: u8 = 0x40;
const M0_SIDE_SHIFT_BIT: u8 = 0x80;
const M0_PAD_MAX_VALUE: u16 = 0x0c40;
const M0_TX_FAIL_RESET_THRESHOLD: u8 = 4;
const M0_LED_BRIGHTNESS_RAW: [u8; 8] = [0, 36, 72, 109, 145, 182, 218, 255];
const M0_LED_BOOST_MIN_LEVEL: u8 = 7;
const M0_MODE_HELD_RESET: u8 = 0;
const M0_MODE_ROM: u8 = 1;
const M0_MODE_FIRMWARE: u8 = 2;
const M0_ROM_ACK: u8 = 0x79;
const M0_ROM_NACK: u8 = 0x1f;
const M0_ROM_SYNC: u8 = 0x7f;
const M0_ROM_CMD_GET: u8 = 0x00;
const M0_ROM_CMD_GET_ID: u8 = 0x02;
const M0_ROM_CMD_READ_MEMORY: u8 = 0x11;
const M0_ROM_FLASH_BASE: u32 = 0x0800_0000;
const M0_ROM_BLID_ADDR: u32 = 0x1fff_f7a6;
const M0_ROM_ACK_TIMEOUT_MS: u64 = 100;
const M0_ROM_READ_TIMEOUT_MS: u64 = 250;
const M0_ROM_MAX_LEN: usize = 256;
const UART5_PCLK_HZ: u32 = 54_000_000;
const UART5_RDR_ADDR: u32 = 0x4000_5024;
const RCC_AHB1ENR: *mut u32 = 0x4002_3830 as *mut u32;
const RCC_AHB1ENR_DMA1EN: u32 = 1 << 21;
const DMA1_LISR: *const u32 = 0x4002_6000 as *const u32;
const DMA1_LIFCR: *mut u32 = 0x4002_6008 as *mut u32;
const DMA1_S0CR: *mut u32 = 0x4002_6010 as *mut u32;
const DMA1_S0NDTR: *mut u32 = 0x4002_6014 as *mut u32;
const DMA1_S0PAR: *mut u32 = 0x4002_6018 as *mut u32;
const DMA1_S0M0AR: *mut u32 = 0x4002_601C as *mut u32;
const DMA1_S0FCR: *mut u32 = 0x4002_6024 as *mut u32;
const DMA_SCR_EN: u32 = 1 << 0;
const DMA_SCR_MINC: u32 = 1 << 10;
const DMA_SCR_PL_VERY_HIGH: u32 = 0b11 << 16;
const DMA_SCR_CHSEL_4: u32 = 0b100 << 25;
const DMA_STREAM0_FLAGS: u32 = (1 << 0) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5);
const DMA_STREAM0_TCIF: u32 = 1 << 5;
const SCB_CCR: *const u32 = 0xE000_ED14 as *const u32;
const SCB_CCR_DC: u32 = 1 << 16;
const SCB_DCIMVAC: *mut u32 = 0xE000_EF5C as *mut u32;
const SCB_DCCIMVAC: *mut u32 = 0xE000_EF70 as *mut u32;
const DCACHE_LINE_BYTES: usize = 32;
pub const M0_ROM_STATUS_OK: u8 = 0;
pub const M0_ROM_STATUS_SYNC: u8 = 2;
pub const M0_ROM_STATUS_CMD: u8 = 3;
pub const M0_ROM_STATUS_NACK: u8 = 4;
pub const M0_ROM_STATUS_RX: u8 = 5;
pub const M0_ROM_STATUS_ARG: u8 = 6;
pub const M0_FW_LEGACY: u8 = 0;
pub const M0_FW_ROADRUNNER: u8 = 1;
pub const M0_FW_UNKNOWN: u8 = 2;
pub const M0_FW_ERROR: u8 = 0x7f;

const M0_ORIGINAL_VECTOR: [u8; 16] = [
    0xD8, 0x0D, 0x00, 0x20, 0x85, 0x26, 0x00, 0x08, 0x29, 0x24, 0x00, 0x08, 0x2B, 0x24, 0x00, 0x08,
];
const ROADRUNNER_INQUIRY_OPCODE: u8 = 0xCC;
const ROADRUNNER_INQUIRY_SCHEMA: u8 = 0x01;
const ROADRUNNER_INQUIRY: [u8; 5] = [
    ROADRUNNER_INQUIRY_OPCODE,
    ROADRUNNER_INQUIRY_SCHEMA,
    0,
    0,
    0,
];
const ROADRUNNER_STATS_OPCODE: u8 = 0xDD;
const ROADRUNNER_STATS_SCHEMA: u8 = 0x01;
const ROADRUNNER_STATS_INQUIRY: [u8; 5] =
    [ROADRUNNER_STATS_OPCODE, ROADRUNNER_STATS_SCHEMA, 0, 0, 0];
const ROADRUNNER_STATS_RESPONSE_LEN: usize = 14;

pub use crate::sys::driver::{FlashInfo, M0FirmwareStatus, M0ProbeResult, RoadrunnerStats};

#[derive(Clone, Copy)]
pub struct M0LinkStatus {
    pub mode: u8,
    pub ready: bool,
    pub stream_synced: bool,
    pub ever_synced: bool,
    pub active_baud: u32,
}

#[repr(align(32))]
struct M0ButtonFrame {
    bytes: [u8; M0_BTN_FRAME_DMA_SIZE],
}

impl M0ButtonFrame {
    const fn new() -> Self {
        Self {
            bytes: [0; M0_BTN_FRAME_DMA_SIZE],
        }
    }
}

pub struct RuntimeDriver {
    link: M0Link,
    leds: LedSystem,
    flash: ExtFlash<'static>,
    brightness: u8,
    m0_firmware_kind: u8,
    m0_firmware_status: M0FirmwareStatus,
}

impl RuntimeDriver {
    pub fn new(
        uart5: Peri<'static, peripherals::UART5>,
        rx: Peri<'static, peripherals::PD2>,
        tx: Peri<'static, peripherals::PC12>,
        boot0: Peri<'static, peripherals::PA4>,
        reset: Peri<'static, peripherals::PA6>,
        setup: Peri<'static, peripherals::PA2>,
        shift: Peri<'static, peripherals::PA3>,
        flash: ExtFlash<'static>,
    ) -> Self {
        let link = M0Link::new(uart5, rx, tx, boot0, reset, setup, shift);
        Self {
            link,
            leds: LedSystem::new_legacy(),
            flash,
            brightness: 7,
            m0_firmware_kind: M0_FW_UNKNOWN,
            m0_firmware_status: M0FirmwareStatus::unknown(),
        }
    }

    pub fn poll(&mut self) -> Option<SurfaceEvent> {
        self.link.poll()
    }

    pub fn leds_task(&mut self) {
        self.leds.task(&mut self.link);
    }

    pub fn is_ready(&self) -> bool {
        self.link.is_ready()
    }

    pub fn refresh_m0_firmware_status(&mut self) -> M0FirmwareStatus {
        let status = self.link.firmware_status();
        self.apply_m0_firmware_status(&status);
        status
    }

    pub fn detect_m0_firmware_before_stream(&mut self, timeout_ms: u64) -> M0FirmwareStatus {
        let status = self.link.firmware_status_running(timeout_ms);
        self.apply_m0_firmware_status(&status);
        status
    }

    pub fn confirm_legacy_m0_firmware(&mut self) {
        let status = M0FirmwareStatus {
            status: M0_ROM_STATUS_OK,
            kind: M0_FW_LEGACY,
            version_major: 0,
            version_minor: 0,
            version_patch: 0,
            probe: M0ProbeResult::new(),
        };
        self.apply_m0_firmware_status(&status);
    }

    fn apply_m0_firmware_status(&mut self, status: &M0FirmwareStatus) {
        self.m0_firmware_status = *status;
        match status.kind {
            M0_FW_ROADRUNNER => {
                self.m0_firmware_kind = M0_FW_ROADRUNNER;
                self.leds = LedSystem::new_highspeed();
            }
            M0_FW_LEGACY => {
                self.m0_firmware_kind = M0_FW_LEGACY;
                self.leds = LedSystem::new_legacy();
            }
            _ => {
                if self.m0_firmware_kind == M0_FW_UNKNOWN {
                    self.leds = LedSystem::new_legacy();
                }
            }
        }
    }
}

impl Driver for RuntimeDriver {
    fn set_rgb_led(&mut self, index: u8, r: u8, g: u8, b: u8) {
        self.leds.set_rgb_led(index, r, g, b);
    }

    fn set_led(&mut self, index: u8, color: u32) {
        self.leds.set_led(index, color);
    }

    fn fill(&mut self, color: u32) {
        for led in 0..LED_REMAP_SIZE as u8 {
            self.leds.set_led(led, color);
        }
    }

    fn brightness(&mut self) -> u8 {
        self.brightness
    }

    fn set_brightness(&mut self, brightness: u8) {
        self.brightness = brightness.min(7);
        self.link.set_led_brightness(self.brightness);
    }

    fn highspeed_leds_enabled(&self) -> bool {
        self.m0_firmware_kind == M0_FW_ROADRUNNER
    }

    fn send_midi(&mut self, port: crate::sys::midi::MidiPort, data: &[u8]) {
        let _ = super::usb::enqueue_tx_message(port.as_cable(), data);
    }

    fn flash_size(&mut self) -> u32 {
        self.flash.capacity() as u32
    }

    fn read_flash(&mut self, offset: u32, data: &mut [u8]) {
        if self.flash.read(offset, data).is_err() {
            data.fill(0xff);
        }
    }

    fn write_flash(&mut self, offset: u32, data: &[u8]) {
        let _ = self.flash.write(offset, data);
    }

    fn prepare_settings_storage(&mut self, wire_size: usize, validate: fn(&[u8]) -> bool) {
        let _ = self.flash.migrate_legacy_settings(wire_size, validate);
    }

    fn device_id(&self) -> u8 {
        35
    }

    fn cached_m0_firmware_status(&mut self) -> Option<M0FirmwareStatus> {
        Some(self.m0_firmware_status)
    }

    fn refresh_m0_firmware_status(&mut self) -> Option<M0FirmwareStatus> {
        Some(self.refresh_m0_firmware_status())
    }

    fn flash_info(&mut self) -> Option<FlashInfo> {
        let info = self.flash.info();
        Some(FlashInfo {
            present: info.present,
            jedec_id: info.jedec_id,
            status1: info.status1,
        })
    }

    fn roadrunner_stats(&mut self) -> Option<Option<RoadrunnerStats>> {
        Some(self.link.roadrunner_stats_inquiry(100))
    }
}

pub struct M0Link {
    uart: Uart<'static, Blocking>,
    boot0: Output<'static>,
    reset: Output<'static>,
    setup: Input<'static>,
    shift: Input<'static>,
    ready: bool,
    stream_synced: bool,
    last_retry_ms: u64,
    last_frame_ms: u64,
    active_baud: u32,
    mode: u8,
    variant_idx: usize,
    tx_fail_streak: u8,
    stream_miss_streak: u8,
    stream_warmup_frames: u8,
    led_brightness_raw: u8,
    led_boost_enabled: u8,
    pub led_tx_slots: u8,
    link_ever_synced: bool,
    input_filter: M0InputFilter,
    pad_pressed: [bool; M0_PAD_COUNT],
    side_pressed: [bool; M0_SIDE_COUNT],
    frame: M0ButtonFrame,
}

impl M0Link {
    fn new(
        uart5: Peri<'static, peripherals::UART5>,
        rx: Peri<'static, peripherals::PD2>,
        tx: Peri<'static, peripherals::PC12>,
        boot0: Peri<'static, peripherals::PA4>,
        reset: Peri<'static, peripherals::PA6>,
        setup: Peri<'static, peripherals::PA2>,
        shift: Peri<'static, peripherals::PA3>,
    ) -> Self {
        let mut config = UartConfig::default();
        config.baudrate = M0_UART_BAUD_FIRMWARE;
        let uart = Uart::new_blocking(uart5, rx, tx, config).expect("UART5 init");
        set_m0_uart_gpio_speed();

        let mut boot0 = Output::new(boot0, Level::Low, Speed::Low);
        let mut reset = Output::new(reset, Level::High, Speed::Low);

        boot0.set_low();
        reset_pulse(&mut reset);

        Self {
            uart,
            boot0,
            reset,
            setup: Input::new(setup, Pull::None),
            shift: Input::new(shift, Pull::None),
            ready: false,
            stream_synced: false,
            link_ever_synced: false,
            last_retry_ms: now_ms().saturating_sub(M0_RETRY_INTERVAL_COLD_MS),
            last_frame_ms: now_ms(),
            active_baud: M0_UART_BAUD_FIRMWARE,
            mode: M0_MODE_FIRMWARE,
            variant_idx: 0,
            tx_fail_streak: 0,
            stream_miss_streak: 0,
            stream_warmup_frames: M0_STREAM_WARMUP_FRAMES,
            led_brightness_raw: M0_LED_BRIGHTNESS_RAW[7],
            led_boost_enabled: 1,
            led_tx_slots: 0,
            input_filter: M0InputFilter::new(),
            pad_pressed: [false; M0_PAD_COUNT],
            side_pressed: [false; M0_SIDE_COUNT],
            frame: M0ButtonFrame::new(),
        }
    }

    fn poll(&mut self) -> Option<SurfaceEvent> {
        if self.ready {
            return self.buttons_poll();
        }

        let now = now_ms();
        if now.saturating_sub(self.last_retry_ms) >= self.retry_interval_ms() {
            self.last_retry_ms = now;
            if self.try_handshake() {
                self.ready = true;
                self.boot0.set_low();
            }
        }

        None
    }

    pub fn is_ready(&self) -> bool {
        self.ready && self.stream_synced
    }

    pub fn set_mode(&mut self, mode: u8) -> u8 {
        self.ready = false;
        self.stream_synced = false;
        self.led_tx_slots = 0;
        self.tx_fail_streak = 0;
        self.stream_miss_streak = 0;
        self.stream_warmup_frames = M0_STREAM_WARMUP_FRAMES;
        self.input_filter.reset_stream();
        self.drain_rx();

        match mode {
            M0_MODE_HELD_RESET => {
                self.boot0.set_low();
                self.reset.set_low();
                self.mode = mode;
                M0_ROM_STATUS_OK
            }
            M0_MODE_ROM => {
                self.boot0.set_high();
                reset_pulse(&mut self.reset);
                self.mode = mode;
                M0_ROM_STATUS_OK
            }
            M0_MODE_FIRMWARE => {
                self.boot0.set_low();
                reset_pulse(&mut self.reset);
                self.mode = mode;
                self.last_retry_ms = now_ms().saturating_sub(self.retry_interval_ms());
                M0_ROM_STATUS_OK
            }
            _ => M0_ROM_STATUS_ARG,
        }
    }

    pub fn status(&self) -> M0LinkStatus {
        M0LinkStatus {
            mode: self.mode,
            ready: self.ready,
            stream_synced: self.stream_synced,
            ever_synced: self.link_ever_synced,
            active_baud: self.active_baud,
        }
    }

    pub fn firmware_status(&mut self) -> M0FirmwareStatus {
        if let Some((major, minor, patch)) = self.roadrunner_version_inquiry(500) {
            return M0FirmwareStatus {
                status: M0_ROM_STATUS_OK,
                kind: M0_FW_ROADRUNNER,
                version_major: major,
                version_minor: minor,
                version_patch: patch,
                probe: M0ProbeResult::new(),
            };
        }

        let probe = self.force_rom_probe();
        if probe.status != M0_ROM_STATUS_OK {
            return M0FirmwareStatus {
                status: probe.status,
                kind: M0_FW_ERROR,
                version_major: 0,
                version_minor: 0,
                version_patch: 0,
                probe,
            };
        }

        if probe.vector_len as usize == M0_ORIGINAL_VECTOR.len()
            && probe.vector == M0_ORIGINAL_VECTOR
        {
            let _ = self.set_mode(M0_MODE_FIRMWARE);
            return M0FirmwareStatus {
                status: M0_ROM_STATUS_OK,
                kind: M0_FW_LEGACY,
                version_major: 0,
                version_minor: 0,
                version_patch: 0,
                probe,
            };
        }

        let _ = self.set_mode(M0_MODE_FIRMWARE);
        if let Some((major, minor, patch)) = self.roadrunner_version_inquiry(500) {
            return M0FirmwareStatus {
                status: M0_ROM_STATUS_OK,
                kind: M0_FW_ROADRUNNER,
                version_major: major,
                version_minor: minor,
                version_patch: patch,
                probe,
            };
        }

        M0FirmwareStatus {
            status: M0_ROM_STATUS_OK,
            kind: M0_FW_UNKNOWN,
            version_major: 0,
            version_minor: 0,
            version_patch: 0,
            probe,
        }
    }

    pub fn firmware_status_running(&mut self, timeout_ms: u64) -> M0FirmwareStatus {
        let start = now_ms();
        let timeout_ms = timeout_ms.max(25);
        while now_ms().saturating_sub(start) < timeout_ms {
            let elapsed = now_ms().saturating_sub(start);
            let remaining = timeout_ms.saturating_sub(elapsed);
            let attempt_timeout = remaining.min(75).max(25);
            if let Some((major, minor, patch)) = self.roadrunner_version_inquiry(attempt_timeout) {
                return M0FirmwareStatus {
                    status: M0_ROM_STATUS_OK,
                    kind: M0_FW_ROADRUNNER,
                    version_major: major,
                    version_minor: minor,
                    version_patch: patch,
                    probe: M0ProbeResult::new(),
                };
            }
            busy_delay_ms(10);
        }

        M0FirmwareStatus {
            status: M0_ROM_STATUS_OK,
            kind: M0_FW_UNKNOWN,
            version_major: 0,
            version_minor: 0,
            version_patch: 0,
            probe: M0ProbeResult::new(),
        }
    }

    pub fn rom_probe(&mut self) -> M0ProbeResult {
        const ROM_BAUDS: [u32; 4] = [115_200, 57_600, 38_400, 9_600];
        let mut result = M0ProbeResult::new();

        for baud in ROM_BAUDS {
            result.baud = baud;
            let mut ack = 0;
            if !self.rom_enter(baud, &mut ack) {
                result.status = M0_ROM_STATUS_SYNC;
                result.ack = ack;
                continue;
            }

            let mut status = M0_ROM_STATUS_OK;
            let Some(pid) = self.rom_get_id(&mut ack, &mut status) else {
                result.status = status;
                result.ack = ack;
                continue;
            };

            result.status = M0_ROM_STATUS_OK;
            result.ack = ack;
            result.pid = pid;

            let mut blid = [0u8; 1];
            let read_status = self.rom_read(M0_ROM_BLID_ADDR, &mut blid);
            if read_status == M0_ROM_STATUS_OK {
                result.blid = blid[0];
                result.read_status = M0_ROM_STATUS_OK;
            } else {
                result.read_status = read_status;
            }

            let mut vector = [0u8; 16];
            let vector_status = self.rom_read(M0_ROM_FLASH_BASE, &mut vector);
            if vector_status == M0_ROM_STATUS_OK {
                result.vector = vector;
                result.vector_len = vector.len() as u8;
                result.read_status = M0_ROM_STATUS_OK;
            } else if result.read_status == M0_ROM_STATUS_OK {
                result.read_status = vector_status;
            }

            return result;
        }

        result
    }

    pub fn force_rom_probe(&mut self) -> M0ProbeResult {
        let _ = self.set_mode(M0_MODE_ROM);
        busy_delay_ms(50);
        self.rom_probe()
    }

    pub fn rom_read(&mut self, addr: u32, data: &mut [u8]) -> u8 {
        if data.is_empty() || data.len() > M0_ROM_MAX_LEN {
            return M0_ROM_STATUS_ARG;
        }

        let mut ack = 0;
        let mut status = M0_ROM_STATUS_OK;
        if !self.rom_cmd(M0_ROM_CMD_READ_MEMORY, &mut ack, &mut status) {
            return status;
        }

        let addr_packet = [
            (addr >> 24) as u8,
            (addr >> 16) as u8,
            (addr >> 8) as u8,
            addr as u8,
            rom_addr_xor(addr),
        ];
        if !self.rom_tx(&addr_packet) || !self.rom_wait_ack(&mut ack) {
            return ack_status(ack, M0_ROM_STATUS_RX);
        }

        let len = (data.len() - 1) as u8;
        if !self.rom_tx(&[len, !len]) || !self.rom_wait_ack(&mut ack) {
            return ack_status(ack, M0_ROM_STATUS_RX);
        }

        if !self.rom_rx(data, M0_ROM_READ_TIMEOUT_MS) {
            return M0_ROM_STATUS_RX;
        }

        M0_ROM_STATUS_OK
    }

    pub fn rom_get_commands(&mut self, data: &mut [u8]) -> Result<usize, u8> {
        if data.len() < M0_ROM_MAX_LEN {
            return Err(M0_ROM_STATUS_ARG);
        }

        let mut ack = 0;
        let mut status = M0_ROM_STATUS_OK;
        if !self.rom_cmd(M0_ROM_CMD_GET, &mut ack, &mut status) {
            return Err(status);
        }

        let mut n = [0u8; 1];
        if !self.rom_rx(&mut n, M0_ROM_ACK_TIMEOUT_MS) {
            return Err(M0_ROM_STATUS_RX);
        }

        let len = n[0] as usize + 1;
        if len > data.len() || !self.rom_rx(&mut data[..len], M0_ROM_ACK_TIMEOUT_MS) {
            return Err(M0_ROM_STATUS_RX);
        }

        if !self.rom_wait_ack(&mut ack) {
            return Err(ack_status(ack, M0_ROM_STATUS_RX));
        }

        Ok(len)
    }

    fn try_handshake(&mut self) -> bool {
        const VARIANTS: [u32; 4] = [
            M0_UART_BAUD_FIRMWARE,
            M0_UART_BAUD_BOOTLOADER,
            M0_UART_BAUD_BOOTLOADER,
            M0_UART_BAUD_FIRMWARE,
        ];

        self.stream_synced = false;
        self.input_filter.reset_stream();
        self.active_baud = VARIANTS[self.variant_idx];
        let mut config = UartConfig::default();
        config.baudrate = self.active_baud;
        let _ = self.uart.set_config(&config);

        self.mode = M0_MODE_FIRMWARE;
        self.boot0.set_low();
        self.drain_rx();

        if self.send_ping_once() {
            busy_delay_ms(self.handshake_settle_ms());
            if !self.apply_led_config() {
                self.next_variant();
                return false;
            }
            busy_delay_ms(self.handshake_settle_ms());
            self.last_frame_ms = now_ms();
            return true;
        }

        self.next_variant();
        false
    }

    fn next_variant(&mut self) {
        self.variant_idx = (self.variant_idx + 1) % 4;
    }

    fn retry_interval_ms(&self) -> u64 {
        if self.link_ever_synced {
            M0_RETRY_INTERVAL_RUNTIME_MS
        } else {
            M0_RETRY_INTERVAL_COLD_MS
        }
    }

    fn handshake_settle_ms(&self) -> u64 {
        if self.link_ever_synced {
            M0_HANDSHAKE_SETTLE_RETRY_MS
        } else {
            M0_HANDSHAKE_SETTLE_COLD_MS
        }
    }

    fn schedule_retry(&mut self) {
        let now = now_ms();
        self.last_retry_ms = now.saturating_sub(self.retry_interval_ms());
    }

    fn send_ping_once(&mut self) -> bool {
        if !self.send_raw(&[0xbb, 0, 0, 0, 0]) {
            return false;
        }

        let start = now_ms();
        while now_ms().saturating_sub(start) < 25 {
            if let Some(byte) = self.read_byte() {
                if byte == 0xbb {
                    return true;
                }
            }
        }
        false
    }

    fn roadrunner_version_inquiry(&mut self, timeout_ms: u64) -> Option<(u8, u8, u8)> {
        self.active_baud = M0_UART_BAUD_FIRMWARE;
        let mut config = UartConfig::default();
        config.baudrate = self.active_baud;
        let _ = self.uart.set_config(&config);
        self.boot0.set_low();
        self.drain_rx();
        if !self.send_raw(&ROADRUNNER_INQUIRY) {
            return None;
        }

        let mut response = [0u8; 5];
        let mut pos = 0usize;
        let start = now_ms();
        while now_ms().saturating_sub(start) < timeout_ms {
            if let Some(byte) = self.read_byte() {
                if pos == 0 && byte != ROADRUNNER_INQUIRY_OPCODE {
                    continue;
                }
                response[pos] = byte;
                pos += 1;
                if pos == response.len() {
                    if response[0] == ROADRUNNER_INQUIRY_OPCODE
                        && response[1] == ROADRUNNER_INQUIRY_SCHEMA
                    {
                        return Some((response[2], response[3], response[4]));
                    }
                    return None;
                }
            }
        }
        None
    }

    fn roadrunner_stats_inquiry(&mut self, timeout_ms: u64) -> Option<RoadrunnerStats> {
        if !self.send_raw(&ROADRUNNER_STATS_INQUIRY) {
            return None;
        }

        let mut response = [0u8; ROADRUNNER_STATS_RESPONSE_LEN];
        let mut pos = 0usize;
        let start = now_ms();
        while now_ms().saturating_sub(start) < timeout_ms {
            if let Some(byte) = self.read_byte() {
                if pos == 0 && byte != ROADRUNNER_STATS_OPCODE {
                    continue;
                }
                response[pos] = byte;
                pos += 1;
                if pos == response.len() {
                    if response[0] != ROADRUNNER_STATS_OPCODE
                        || response[1] != ROADRUNNER_STATS_SCHEMA
                    {
                        return None;
                    }
                    return Some(RoadrunnerStats {
                        fast_frames: u32::from_le_bytes([
                            response[2],
                            response[3],
                            response[4],
                            response[5],
                        ]),
                        commits: u32::from_le_bytes([
                            response[6],
                            response[7],
                            response[8],
                            response[9],
                        ]),
                        rx_overruns: u32::from_le_bytes([
                            response[10],
                            response[11],
                            response[12],
                            response[13],
                        ]),
                    });
                }
            }
        }
        None
    }

    fn buttons_poll(&mut self) -> Option<SurfaceEvent> {
        self.led_tx_slots = 0;
        clear_m0_uart_errors();
        start_m0_button_dma(&mut self.frame);
        if !self.send_cmd_aa(1, M0_BTN_STREAM_ACK_VALUE) {
            abort_m0_button_dma();
            if self.stream_synced && self.stream_miss_streak < M0_STREAM_MISS_TOLERANCE {
                self.stream_miss_streak += 1;
            } else {
                self.stream_synced = false;
                self.stream_warmup_frames = M0_STREAM_WARMUP_FRAMES;
            }
            return None;
        }

        let timeout_ms = if self.active_baud <= 200_000 {
            M0_BTN_RX_TIMEOUT_SLOW_MS
        } else {
            M0_BTN_RX_TIMEOUT_FAST_MS
        };
        let received = wait_m0_button_dma(&mut self.frame, timeout_ms);

        if received
            && self.frame.bytes[0] == M0_BTN_FRAME_SYNC
            && self.frame.bytes[1] == M0_BTN_FRAME_SYNC
        {
            if !self.stream_synced {
                self.input_filter.reset_stream();
            }
            self.stream_synced = true;
            self.link_ever_synced = true;
            self.stream_miss_streak = 0;
            self.last_frame_ms = now_ms();
            if self.stream_warmup_frames != 0 {
                self.led_tx_slots = M0_LED_TX_WARMUP_SLOTS;
                self.stream_warmup_frames -= 1;
            } else {
                self.led_tx_slots = M0_LED_TX_WINDOW_SLOTS;
            }
            let frame_counter = u16::from_le_bytes([
                self.frame.bytes[2],
                self.frame.bytes[3],
            ]);
            if self.input_filter.accept_frame(frame_counter) {
                return self.process_frame();
            }
            return None;
        }

        if self.stream_synced && self.stream_miss_streak < M0_STREAM_MISS_TOLERANCE {
            self.stream_miss_streak += 1;
        } else {
            self.stream_synced = false;
            self.stream_warmup_frames = M0_STREAM_WARMUP_FRAMES;
            self.stream_miss_streak = 0;
        }
        self.led_tx_slots = 0;
        self.drain_rx();
        if now_ms().saturating_sub(self.last_frame_ms) >= M0_BTN_STALL_RESTART_MS {
            self.ready = false;
            self.stream_synced = false;
            self.schedule_retry();
        }
        None
    }

    fn process_frame(&mut self) -> Option<SurfaceEvent> {
        for i in 0..M0_PAD_COUNT {
            let off = M0_BTN_DATA_OFFSET + 2 * i;
            let raw = u16::from_le_bytes([self.frame.bytes[off], self.frame.bytes[off + 1]]);
            let was = self.pad_pressed[i];
            let is = self.input_filter.pad_state(i, was, raw);

            if is != was {
                self.pad_pressed[i] = is;
                return map_control(i as u8, is, raw);
            }
        }

        for i in 0..M0_SIDE_COUNT {
            let byte_off = M0_BTN_DIGITAL_OFFSET + (i >> 3);
            let bit_mask = 1u8 << (i & 7);
            let mut side_byte = self.frame.bytes[byte_off];
            if (i >> 3) == M0_SIDE_AUX_BYTE_INDEX {
                side_byte &= !(M0_SIDE_SETUP_BIT | M0_SIDE_SHIFT_BIT);
                side_byte |= self.side_aux_bits();
            }

            let is = (side_byte & bit_mask) != 0;
            let was = self.side_pressed[i];
            if is != was {
                self.side_pressed[i] = is;
                return map_control((M0_PAD_COUNT + i) as u8, is, M0_PAD_MAX_VALUE);
            }
        }

        None
    }

    fn side_aux_bits(&self) -> u8 {
        let mut bits = 0;
        if self.setup.is_high() {
            bits |= M0_SIDE_SETUP_BIT;
        }
        if self.shift.is_high() {
            bits |= M0_SIDE_SHIFT_BIT;
        }
        bits
    }

    fn send_cmd_aa(&mut self, cmd: u8, value: u16) -> bool {
        self.send_raw(&[0xaa, cmd, value as u8, (value >> 8) as u8, 0])
    }

    pub fn send_cmd_55(&mut self, cmd: u8, rgb: u32) -> bool {
        let mut frame = [0u8; 5];
        frame[0] = 0x55;
        frame[1] = cmd;
        frame[2] = (rgb >> 16) as u8;
        frame[3] = (rgb >> 8) as u8;
        frame[4] = rgb as u8;
        self.send_raw(&frame)
    }

    pub fn send_led_frame(&mut self, fb: &[u8; 264]) -> bool {
        if !self.is_ready() || self.led_tx_slots == 0 {
            return false;
        }
        let mut frame = [0u8; 265];
        frame[0] = 0x88;
        frame[1..].copy_from_slice(fb);
        if self.send_raw(&frame) {
            self.led_tx_slots = self.led_tx_slots.saturating_sub(1);
            true
        } else {
            false
        }
    }

    fn send_cmd_66(&mut self, value: u8) -> bool {
        self.send_raw(&[0x66, value >> 5, 0, 0, 0])
    }

    fn send_cmd_77(&mut self, value: u8) -> bool {
        self.send_raw(&[0x77, value, 0, 0, 0])
    }

    pub fn set_led_brightness(&mut self, level: u8) {
        let level = level.min(7);
        self.led_brightness_raw = M0_LED_BRIGHTNESS_RAW[level as usize];
        self.led_boost_enabled = if level >= M0_LED_BOOST_MIN_LEVEL {
            1
        } else {
            0
        };
        if self.ready {
            let _ = self.apply_led_config();
        }
    }

    fn apply_led_config(&mut self) -> bool {
        self.send_cmd_77(self.led_boost_enabled) && self.send_cmd_66(self.led_brightness_raw)
    }

    fn send_raw(&mut self, data: &[u8]) -> bool {
        clear_m0_uart_errors();
        let ok = self.uart.blocking_write(data).is_ok() && self.uart.blocking_flush().is_ok();
        if ok {
            self.tx_fail_streak = 0;
            cortex_m::asm::delay(3_200);
            clear_m0_uart_errors();
            true
        } else {
            self.tx_fail_streak = self.tx_fail_streak.saturating_add(1);
            self.stream_synced = false;
            self.drain_rx();
            if self.tx_fail_streak >= M0_TX_FAIL_RESET_THRESHOLD {
                self.ready = false;
                self.stream_synced = false;
                self.schedule_retry();
            }
            clear_m0_uart_errors();
            false
        }
    }

    fn read_byte(&mut self) -> Option<u8> {
        match self.uart.read() {
            Ok(byte) => Some(byte),
            Err(nb::Error::WouldBlock) => None,
            Err(nb::Error::Other(_)) => {
                clear_m0_uart_errors();
                None
            }
        }
    }

    fn drain_rx(&mut self) {
        for _ in 0..M0_BTN_FRAME_SIZE {
            if self.read_byte().is_none() {
                break;
            }
        }
    }

    fn rom_enter(&mut self, baud: u32, ack: &mut u8) -> bool {
        *ack = 0;
        self.set_mode(M0_MODE_ROM);
        raw_uart_apply_rom(baud);
        self.active_baud = baud;
        self.drain_rx();

        for _ in 0..5 {
            if self.rom_tx(&[M0_ROM_SYNC]) && self.rom_rx(core::slice::from_mut(ack), 20) {
                if *ack == M0_ROM_ACK {
                    return true;
                }
            }
            self.drain_rx();
            busy_delay_ms(10);
        }

        false
    }

    fn rom_tx(&mut self, data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }
        raw_uart_tx(data)
    }

    fn rom_rx(&mut self, data: &mut [u8], timeout_ms: u64) -> bool {
        if data.is_empty() {
            return false;
        }

        raw_uart_rx(data, timeout_ms)
    }

    fn rom_wait_ack(&mut self, ack: &mut u8) -> bool {
        self.rom_wait_ack_timeout(ack, M0_ROM_ACK_TIMEOUT_MS)
    }

    fn rom_wait_ack_timeout(&mut self, ack: &mut u8, timeout_ms: u64) -> bool {
        *ack = 0;
        if !self.rom_rx(core::slice::from_mut(ack), timeout_ms) {
            return false;
        }
        *ack == M0_ROM_ACK
    }

    fn rom_cmd(&mut self, cmd: u8, ack: &mut u8, status: &mut u8) -> bool {
        *status = M0_ROM_STATUS_OK;
        if !self.rom_tx(&[cmd, cmd ^ 0xff]) || !self.rom_wait_ack(ack) {
            *status = if *ack == M0_ROM_NACK {
                M0_ROM_STATUS_NACK
            } else {
                M0_ROM_STATUS_CMD
            };
            return false;
        }
        true
    }

    fn rom_get_id(&mut self, ack: &mut u8, status: &mut u8) -> Option<u16> {
        if !self.rom_cmd(M0_ROM_CMD_GET_ID, ack, status) {
            return None;
        }

        let mut n = [0u8; 1];
        if !self.rom_rx(&mut n, M0_ROM_ACK_TIMEOUT_MS) || n[0] != 1 {
            *status = M0_ROM_STATUS_RX;
            return None;
        }

        let mut id = [0u8; 2];
        if !self.rom_rx(&mut id, M0_ROM_ACK_TIMEOUT_MS) || !self.rom_wait_ack(ack) {
            *status = ack_status(*ack, M0_ROM_STATUS_RX);
            return None;
        }

        Some(u16::from_be_bytes(id))
    }
}

fn map_control(control: u8, pressed: bool, raw: u16) -> Option<SurfaceEvent> {
    if control as usize >= BUTTON_REMAP_SIZE {
        return None;
    }
    let note = BUTTON_REMAP[control as usize];
    if note == 0 && control != 78 {
        return None;
    }
    Some(SurfaceEvent {
        pressed,
        index: note,
        value: if pressed { raw_to_velocity(raw) } else { 0 },
    })
}

fn raw_to_velocity(raw: u16) -> u8 {
    if raw <= M0_PAD_PRESS_THRESHOLD {
        return 1;
    }
    if raw >= M0_PAD_MAX_VALUE {
        return 127;
    }

    let span = (M0_PAD_MAX_VALUE - M0_PAD_PRESS_THRESHOLD) as u32;
    1 + (((raw - M0_PAD_PRESS_THRESHOLD) as u32 * 126) / span) as u8
}

fn now_ms() -> u64 {
    embassy_time::Instant::now().as_millis()
}

fn busy_delay_ms(ms: u64) {
    let start = now_ms();
    while now_ms().saturating_sub(start) < ms {}
}

fn reset_pulse(reset: &mut Output<'static>) {
    reset.set_low();
    busy_delay_ms(50);
    reset.set_high();
    busy_delay_ms(M0_RESET_SETTLE_MS);
}

fn raw_uart_apply_rom(baud: u32) {
    use stm32_metapac as pac;
    use stm32_metapac::usart::vals::{Over8, Ps, Stop, M0};

    let uart = pac::UART5;

    uart.cr1().modify(|w| {
        w.set_ue(false);
        w.set_re(false);
        w.set_te(false);
    });

    uart.cr2().write(|w| {
        w.set_stop(Stop::STOP1);
    });
    uart.cr3().write(|_| {});
    uart.brr().write(|w| {
        w.set_brr(uart_brr_oversampling16(baud));
    });
    uart.cr1().write(|w| {
        w.set_m0(M0::BIT9);
        w.set_pce(true);
        w.set_ps(Ps::EVEN);
        w.set_over8(Over8::OVERSAMPLING16);
        w.set_re(true);
        w.set_te(true);
        w.set_ue(true);
    });

    clear_m0_uart_errors();
}

fn raw_uart_tx(data: &[u8]) -> bool {
    use stm32_metapac as pac;

    if data.is_empty() {
        return false;
    }

    clear_m0_uart_errors();
    let uart = pac::UART5;
    for &byte in data {
        let start = now_ms();
        while !uart.isr().read().txe() {
            if now_ms().saturating_sub(start) >= 20 {
                return false;
            }
        }
        uart.tdr().write(|w| w.set_dr(byte as u16));
    }

    let start = now_ms();
    while !uart.isr().read().tc() {
        if now_ms().saturating_sub(start) >= 20 {
            return false;
        }
    }
    true
}

fn raw_uart_rx(data: &mut [u8], timeout_ms: u64) -> bool {
    use stm32_metapac as pac;

    if data.is_empty() {
        return false;
    }

    clear_m0_uart_errors();
    let uart = pac::UART5;
    let start = now_ms();
    let mut pos = 0usize;
    while pos < data.len() && now_ms().saturating_sub(start) < timeout_ms {
        let isr = uart.isr().read();
        if isr.rxne() {
            data[pos] = uart.rdr().read().dr() as u8;
            pos += 1;
            if isr.pe() || isr.fe() || isr.ne() || isr.ore() {
                clear_m0_uart_errors();
            }
            continue;
        }
        if isr.pe() || isr.fe() || isr.ne() || isr.ore() {
            clear_m0_uart_errors();
        }
    }

    pos == data.len()
}

fn start_m0_button_dma(frame: &mut M0ButtonFrame) {
    use stm32_metapac as pac;

    abort_m0_button_dma();
    clean_invalidate_dcache(frame);
    unsafe {
        core::ptr::write_volatile(DMA1_LIFCR, DMA_STREAM0_FLAGS);
        core::ptr::write_volatile(DMA1_S0PAR, UART5_RDR_ADDR);
        core::ptr::write_volatile(DMA1_S0M0AR, frame.bytes.as_mut_ptr() as u32);
        core::ptr::write_volatile(DMA1_S0NDTR, M0_BTN_FRAME_SIZE as u32);
        core::ptr::write_volatile(DMA1_S0FCR, 0);
        core::ptr::write_volatile(
            DMA1_S0CR,
            DMA_SCR_CHSEL_4 | DMA_SCR_PL_VERY_HIGH | DMA_SCR_MINC,
        );
    }
    pac::UART5.cr3().modify(|w| w.set_dmar(true));
    unsafe {
        core::ptr::write_volatile(
            DMA1_S0CR,
            DMA_SCR_CHSEL_4 | DMA_SCR_PL_VERY_HIGH | DMA_SCR_MINC | DMA_SCR_EN,
        );
    }
}

fn wait_m0_button_dma(frame: &mut M0ButtonFrame, timeout_ms: u64) -> bool {
    let start = now_ms();
    while now_ms().saturating_sub(start) < timeout_ms {
        let flags = unsafe { core::ptr::read_volatile(DMA1_LISR) };
        if (flags & DMA_STREAM0_TCIF) != 0 {
            abort_m0_button_dma();
            invalidate_dcache(frame);
            return true;
        }
    }
    abort_m0_button_dma();
    clear_m0_uart_errors();
    false
}

fn abort_m0_button_dma() {
    use stm32_metapac as pac;

    unsafe {
        let cr = core::ptr::read_volatile(DMA1_S0CR);
        if (cr & DMA_SCR_EN) != 0 {
            core::ptr::write_volatile(DMA1_S0CR, cr & !DMA_SCR_EN);
            while (core::ptr::read_volatile(DMA1_S0CR) & DMA_SCR_EN) != 0 {}
        }
        core::ptr::write_volatile(DMA1_LIFCR, DMA_STREAM0_FLAGS);
    }
    pac::UART5.cr3().modify(|w| w.set_dmar(false));
}

fn clean_invalidate_dcache(frame: &mut M0ButtonFrame) {
    dcache_maintain_by_addr(
        frame.bytes.as_mut_ptr() as usize,
        frame.bytes.len(),
        SCB_DCCIMVAC,
    );
}

fn invalidate_dcache(frame: &mut M0ButtonFrame) {
    dcache_maintain_by_addr(
        frame.bytes.as_mut_ptr() as usize,
        frame.bytes.len(),
        SCB_DCIMVAC,
    );
}

fn dcache_maintain_by_addr(addr: usize, len: usize, register: *mut u32) {
    if unsafe { core::ptr::read_volatile(SCB_CCR) } & SCB_CCR_DC == 0 {
        return;
    }

    let start = addr & !(DCACHE_LINE_BYTES - 1);
    let end = (addr + len + DCACHE_LINE_BYTES - 1) & !(DCACHE_LINE_BYTES - 1);
    cortex_m::asm::dsb();
    let mut line = start;
    while line < end {
        unsafe {
            core::ptr::write_volatile(register, line as u32);
        }
        line += DCACHE_LINE_BYTES;
    }
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

fn uart_brr_oversampling16(baud: u32) -> u16 {
    ((UART5_PCLK_HZ + (baud / 2)) / baud) as u16
}

fn rom_addr_xor(addr: u32) -> u8 {
    ((addr >> 24) ^ (addr >> 16) ^ (addr >> 8) ^ addr) as u8
}

fn ack_status(ack: u8, fallback: u8) -> u8 {
    if ack == M0_ROM_NACK {
        M0_ROM_STATUS_NACK
    } else {
        fallback
    }
}

fn set_m0_uart_gpio_speed() {
    use stm32_metapac as pac;
    use stm32_metapac::gpio::vals::Ospeedr;

    unsafe {
        let ahb1enr = core::ptr::read_volatile(RCC_AHB1ENR);
        core::ptr::write_volatile(RCC_AHB1ENR, ahb1enr | RCC_AHB1ENR_DMA1EN);
    }

    pac::GPIOC
        .ospeedr()
        .modify(|w| w.set_ospeedr(12, Ospeedr::VERY_HIGH_SPEED));
    pac::GPIOD
        .ospeedr()
        .modify(|w| w.set_ospeedr(2, Ospeedr::VERY_HIGH_SPEED));
}

fn clear_m0_uart_errors() {
    use stm32_metapac as pac;

    pac::UART5.icr().write(|w| {
        w.set_pe(true);
        w.set_fe(true);
        w.set_ne(true);
        w.set_ore(true);
    });
}
