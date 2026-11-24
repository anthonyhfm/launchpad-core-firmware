// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

pub mod grid;
pub mod inputs;
pub mod leds;
pub mod runtime;
pub mod surface;
pub mod usb;

use crate::sys::driver::common::storage::Flash;

use embassy_executor::Spawner;
use embassy_stm32::interrupt::InterruptExt;
use embassy_stm32::rcc::*;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Ticker};
use crate::app::{AppHost, AppId, SurfaceEvent};
use crate::sys::driver;
use crate::sys::settings;
use static_cell::StaticCell;

const APP_VECTOR_TABLE: u32 = 0x0800_3400;

type SharedAppHost = Mutex<CriticalSectionRawMutex, AppHost>;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
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
        mul: PllMul::MUL8,
    });
    config.rcc.sys = Sysclk::PLL1_P;
    config.rcc.ahb_pre = AHBPrescaler::DIV1;
    config.rcc.apb1_pre = APBPrescaler::DIV2;
    config.rcc.apb2_pre = APBPrescaler::DIV2;

    let _p = embassy_stm32::init(config);

    embassy_stm32::interrupt::TIM4.set_priority(embassy_stm32::interrupt::Priority::P1);

    let grid = GRID.init(grid::Grid::new());
    let flash = Flash::new();
    let runtime_driver = RUNTIME_DRIVER.init(runtime::RuntimeDriver::new(grid, flash));
    driver::install(runtime_driver);
    settings::load();

    init_usb_board();

    usb::init_event_queues();
    usb::init_with_mode(usb::UsbMode::from_u8(settings::get().usb_mode));

    let app_host = APP_HOST.init(Mutex::new(AppHost::new(AppId::Boot)));
    app_host.lock().await.init();
    leds::start_scan(grid as *mut grid::Grid);
    surface::spawn(&spawner, grid as *mut grid::Grid);

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

        while let Some(event) = grid.poll_event() {
            match event {
                inputs::GridEvent::Press { index, value } => {
                    let Some(&index) = inputs::BUTTON_TO_SURFACE_INDEX.get(index as usize) else {
                        continue;
                    };

                    app_host_guard.route_surface_event(SurfaceEvent {
                        pressed: true,
                        index,
                        value,
                    });
                }
                inputs::GridEvent::Release { index } => {
                    let Some(&index) = inputs::BUTTON_TO_SURFACE_INDEX.get(index as usize) else {
                        continue;
                    };

                    app_host_guard.route_surface_event(SurfaceEvent {
                        pressed: false,
                        index,
                        value: 0,
                    });
                }
            }
        }

        while let Some(message) = usb::dequeue_sysex_message() {
            app_host_guard.receive_sysex(message.port, &message.data[..message.len]);
        }
    }
}

fn init_usb_board() {
    const RCC_APB2ENR: *mut u32 = 0x4002_1018 as *mut u32;
    const GPIOA_CRH: *mut u32 = 0x4001_0804 as *mut u32;
    const GPIOA_BSRR: *mut u32 = 0x4001_0810 as *mut u32;
    const GPIOA_BRR: *mut u32 = 0x4001_0814 as *mut u32;
    const IOPAEN: u32 = 1 << 2;
    const PA10_PA12_MODE_MASK: u32 = 0xfff << 8;
    const PA10_OUTPUT_PA11_PA12_FLOATING: u32 = (0x2 << 8) | (0x4 << 12) | (0x4 << 16);

    unsafe {
        core::ptr::write_volatile(RCC_APB2ENR, core::ptr::read_volatile(RCC_APB2ENR) | IOPAEN);
        core::ptr::write_volatile(
            GPIOA_CRH,
            (core::ptr::read_volatile(GPIOA_CRH) & !PA10_PA12_MODE_MASK)
                | PA10_OUTPUT_PA11_PA12_FLOATING,
        );
        core::ptr::write_volatile(GPIOA_BRR, 1 << 10);
        cortex_m::asm::delay(720_000);
        core::ptr::write_volatile(GPIOA_BSRR, 1 << 10);
    }
}
