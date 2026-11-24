// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

pub mod buttons;
pub mod grid;
pub mod led_scan;
pub mod leds;
pub mod runtime;
pub mod usb;

use crate::sys::driver::common::storage::ExtFlash;

use embassy_executor::Spawner;
use embassy_stm32::rcc::*;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Ticker};
use crate::app::{AppHost, AppId, SurfaceEvent};
use crate::sys::driver;
use crate::sys::settings;
use static_cell::StaticCell;

type SharedAppHost = Mutex<CriticalSectionRawMutex, AppHost>;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    static GRID: StaticCell<grid::Grid<'static>> = StaticCell::new();
    static RUNTIME_DRIVER: StaticCell<runtime::RuntimeDriver> = StaticCell::new();
    static APP_HOST: StaticCell<SharedAppHost> = StaticCell::new();

    let mut config = embassy_stm32::Config::default();

    config.rcc.hse = Some(Hse {
        freq: embassy_stm32::time::Hertz(24_000_000),
        mode: HseMode::Oscillator,
    });

    config.rcc.pll_src = PllSource::HSE;

    config.rcc.pll = Some(Pll {
        prediv: PllPreDiv::DIV12,
        mul: PllMul::MUL168,
        divp: Some(PllPDiv::DIV4),
        divq: Some(PllQDiv::DIV7),
        divr: None,
    });

    config.rcc.ahb_pre = AHBPrescaler::DIV1;
    config.rcc.apb1_pre = APBPrescaler::DIV2;
    config.rcc.apb2_pre = APBPrescaler::DIV1;
    config.rcc.sys = Sysclk::PLL1_P;
    config.rcc.mux.clk48sel = mux::Clk48sel::PLL1_Q;

    let p = embassy_stm32::init(config);

    let grid = GRID.init(grid::Grid::new(
        p.SPI2, p.PB13, p.PB15, p.PB14, p.PA1, p.PA4, p.PA8, p.PB0, p.PB1, p.PB2, p.PB8, p.PB10,
        p.PB12,
    ));

    let flash = ExtFlash::new(p.SPI1, p.PA5, p.PA7, p.PA6, p.PA2);
    let runtime_driver = RUNTIME_DRIVER.init(runtime::RuntimeDriver::new(grid, flash));
    driver::install(runtime_driver);
    settings::load();

    usb::init_event_queues();
    usb::init_with_mode(usb::UsbMode::from_u8(settings::get().usb_mode));

    let app_host = APP_HOST.init(Mutex::new(AppHost::new(AppId::Boot)));
    let mut prev_pressed = [false; 100];
    app_host.lock().await.init();

    led_scan::start(grid as *mut grid::Grid<'static>);

    let mut ticker = Ticker::every(Duration::from_millis(1));

    loop {
        ticker.next().await;

        let mut app_host_guard = app_host.lock().await;

        while let Some(event) = usb::dequeue_midi_event() {
            app_host_guard.route_midi_event(event);
        }

        if let Some(frame) = usb::take_dmx_frame() {
            app_host_guard.route_dmx_frame(&frame);
        }

        app_host_guard.route_tick_event();

        if led_scan::take_frame_complete() {
            for index in 0..100 {
                if !grid.button_is_valid(index) {
                    continue;
                }

                let pressed = grid.button_is_pressed(index);
                let was_pressed = &mut prev_pressed[index as usize];

                if pressed != *was_pressed {
                    *was_pressed = pressed;

                    app_host_guard.route_surface_event(SurfaceEvent {
                        pressed: pressed,
                        index: (index % 10) + (9 - (index / 10)) * 10,
                        value: if pressed { 127 } else { 0 },
                    });
                }
            }
        }

        while let Some(message) = usb::dequeue_sysex_message() {
            app_host_guard.receive_sysex(message.port, &message.data[..message.len]);
        }
    }
}
