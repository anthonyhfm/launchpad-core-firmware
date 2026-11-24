// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

pub mod din;
pub mod grid;
pub mod inputs;
pub mod leds;
pub mod runtime;
pub mod usb;

use crate::sys::driver::common::storage::Flash;

use embassy_executor::Spawner;
use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::interrupt::InterruptExt;
use embassy_stm32::rcc::*;
use embassy_stm32::{Peri, peripherals};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Ticker};
use crate::app::{AftertouchEvent, AppHost, AppId, SurfaceEvent};
use crate::sys::driver;
use crate::sys::settings;
use static_cell::StaticCell;
use stm32_metapac as pac;

const APP_VECTOR_TABLE: u32 = 0x0800_6400;
type SharedAppHost = Mutex<ThreadModeRawMutex, AppHost>;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    static GRID: StaticCell<grid::Grid> = StaticCell::new();
    static RUNTIME_DRIVER: StaticCell<runtime::RuntimeDriver> = StaticCell::new();
    static APP_HOST: StaticCell<SharedAppHost> = StaticCell::new();

    unsafe {
        (*cortex_m::peripheral::SCB::PTR)
            .vtor
            .write(APP_VECTOR_TABLE);
    }

    let mut config = embassy_stm32::Config::default();
    config.rcc.hse = Some(Hse {
        freq: embassy_stm32::time::Hertz(6_000_000),
        mode: HseMode::Oscillator,
    });
    config.rcc.pll = Some(Pll {
        src: PllSource::HSE,
        prediv: PllPreDiv::DIV1,
        mul: PllMul::MUL12,
    });
    config.rcc.sys = Sysclk::PLL1_P;
    config.rcc.ahb_pre = AHBPrescaler::DIV1;
    config.rcc.apb1_pre = APBPrescaler::DIV2;
    config.rcc.apb2_pre = APBPrescaler::DIV1;

    let p = embassy_stm32::init(config);

    // embassy leaves its TIM4 time-driver interrupt at the reset-default
    // preemption priority (P0). Our BASEPRI critical section (see
    // `leds::ScanPriorityCriticalSection`) reserves P0 exclusively for the LED
    // scan and only masks P1..P15, so every embassy-managed interrupt must sit
    // at >= P1 to stay covered by critical sections. Bump TIM4 to P1 before any
    // timer alarm can fire concurrently with a critical section.
    embassy_stm32::interrupt::TIM4.set_priority(embassy_stm32::interrupt::Priority::P1);

    let grid = GRID.init(grid::Grid::new());
    let flash = Flash::new();
    let runtime_driver = RUNTIME_DRIVER.init(runtime::RuntimeDriver::new(grid, flash));
    driver::install(runtime_driver);
    settings::load();

    init_usb_board(p.PA10, p.PA11, p.PA12);

    usb::init_event_queues();
    usb::init_with_mode(usb::UsbMode::from_u8(settings::get().usb_mode));

    let app_host = APP_HOST.init(Mutex::new(AppHost::new(AppId::Boot)));
    app_host.lock().await.init();
    leds::start_scan(grid as *mut grid::Grid);
    grid.start_adc_scan();

    let mut ticker = Ticker::every(Duration::from_millis(1));
    let mut tick_200hz_divider = 0u8;

    loop {
        ticker.next().await;

        grid.tick_1khz_collect();

        tick_200hz_divider = (tick_200hz_divider + 1) % 5;
        if tick_200hz_divider == 0 {
            grid.tick_200hz();
        }

        let mut app_host_guard = app_host.lock().await;

        // USB RX is serviced by the PMA interrupt.  Keep MIDI ahead of the
        // surface-event drain so a host note-on/note-off pair is not delayed
        // behind a burst of local pad/aftertouch events.
        while let Some(event) = usb::dequeue_midi_event() {
            app_host_guard.route_midi_event(event);
        }

        if let Some(frame) = usb::take_dmx_frame() {
            app_host_guard.route_dmx_frame(&frame);
        }

        app_host_guard.route_tick_event();

        while let Some(event) = grid.poll_event() {
            match event {
                inputs::GridEvent::Press { index, value } => {
                    app_host_guard.route_surface_event(SurfaceEvent {
                        pressed: true,
                        index,
                        value,
                    });
                }
                inputs::GridEvent::Release { index } => {
                    app_host_guard.route_surface_event(SurfaceEvent {
                        pressed: false,
                        index,
                        value: 0,
                    });
                }
                inputs::GridEvent::Aftertouch { index, value } => {
                    app_host_guard.route_aftertouch_event(AftertouchEvent { index, value });
                }
            }
        }

        while let Some(message) = usb::dequeue_sysex_message() {
            app_host_guard.receive_sysex(message.port, &message.data[..message.len]);
        }

        drop(app_host_guard);

        grid.tick_1khz_start();
    }
}

fn init_usb_board(
    pa10: Peri<'static, peripherals::PA10>,
    pa11: Peri<'static, peripherals::PA11>,
    pa12: Peri<'static, peripherals::PA12>,
) {
    // The USB peripheral itself is still managed by the existing PMA driver,
    // but its board-level reset and pin setup use Embassy's GPIO HAL.
    let mut usb_reset = Output::new(pa10, Level::Low, Speed::Low);
    let usb_dm = Input::new(pa11, Pull::None);
    let usb_dp = Input::new(pa12, Pull::None);
    cortex_m::asm::delay(720_000);
    usb_reset.set_high();

    // Dropping Embassy GPIO drivers returns their pins to a floating state.
    // Keep ownership for the lifetime of the firmware instead.
    core::mem::forget((usb_reset, usb_dm, usb_dp));

    // The selected STM32F103RB package has no Embassy pin token for PD4,
    // though this board's legacy setup drives its GPIO register. Keep this
    // one package-specific exception typed at the PAC level rather than
    // inventing an unsafe peripheral token.
    pac::RCC.apb2enr().modify(|w| w.set_gpioden(true));
    pac::GPIOD.cr(0).modify(|w| {
        w.0 = (w.0 & !(0xf << 16)) | (0x2 << 16);
    });
    pac::GPIOD.bsrr().write_value(pac::gpio::regs::Bsrr(1 << 4));
}
