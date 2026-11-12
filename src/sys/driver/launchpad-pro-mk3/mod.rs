// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister



use crate::sys::driver::common::storage::ExtFlash;

mod input_filter;
pub mod led;
mod map;
mod runtime;
pub mod usb;
use embassy_executor::Spawner;
use embassy_stm32::Config;
use embassy_stm32::rcc::{
    AHBPrescaler, APBPrescaler, Hse, HseMode, Pll, PllMul, PllPDiv, PllPreDiv, PllQDiv, PllRDiv,
    PllSource, Sysclk,
};
use embassy_stm32::time::Hertz;
use crate::app::{AppHost, AppId};
use crate::sys::driver;
use crate::sys::settings;
use static_cell::StaticCell;

use self::runtime::RuntimeDriver;

type AppHostType = AppHost;
static APP_HOST: StaticCell<
    embassy_sync::mutex::Mutex<embassy_sync::blocking_mutex::raw::ThreadModeRawMutex, AppHostType>,
> = StaticCell::new();
pub type SharedAppHost =
    embassy_sync::mutex::Mutex<embassy_sync::blocking_mutex::raw::ThreadModeRawMutex, AppHostType>;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    unsafe {
        core::ptr::write_volatile(0xE000_E010 as *mut u32, 0);
        core::ptr::write_volatile(0xE000_ED04 as *mut u32, 1 << 25);

        let nvic = &*cortex_m::peripheral::NVIC::PTR;
        for i in 0..8 {
            nvic.icer[i].write(0xFFFF_FFFF);
            nvic.icpr[i].write(0xFFFF_FFFF);
        }
    }

    enable_cpu_caches();

    let mut config = Config::default();

    config.rcc.hse = Some(Hse {
        freq: Hertz(24_000_000),
        mode: HseMode::Oscillator,
    });
    config.rcc.pll_src = PllSource::HSE;
    config.rcc.pll = Some(Pll {
        prediv: PllPreDiv::from(24),
        mul: PllMul::MUL432,
        divp: Some(PllPDiv::DIV2),
        divq: Some(PllQDiv::DIV2),
        divr: None,
    });
    config.rcc.pllsai = Some(Pll {
        prediv: PllPreDiv::from(24),
        mul: PllMul::MUL192,
        divp: Some(PllPDiv::DIV4),
        divq: Some(PllQDiv::DIV2),
        divr: Some(PllRDiv::DIV2),
    });
    config.rcc.sys = Sysclk::PLL1_P;
    config.rcc.ahb_pre = AHBPrescaler::DIV1;
    config.rcc.apb1_pre = APBPrescaler::DIV4;
    config.rcc.apb2_pre = APBPrescaler::DIV2;
    config.rcc.mux.clk48sel = embassy_stm32::rcc::mux::Clk48sel::PLLSAI1_P;

    let p = embassy_stm32::init(config);

    unsafe {
        core::ptr::write_volatile(0xE000_ED08 as *mut u32, 0x0801_0000);
        cortex_m::interrupt::enable();
    }

    static RUNTIME_DRIVER: StaticCell<RuntimeDriver> = StaticCell::new();
    let flash = ExtFlash::new(p.SPI1, p.PA5, p.PB5, p.PB4, p.PA15);
    let runtime_driver = RUNTIME_DRIVER.init(RuntimeDriver::new(
        p.UART5, p.PD2, p.PC12, p.PA4, p.PA6, p.PA2, p.PA3, flash,
    ));
    driver::install(runtime_driver);
    settings::load();

    let initial_m0_firmware = runtime_driver.detect_m0_firmware_before_stream(900);
    let m0_start = embassy_time::Instant::now();
    while !runtime_driver.is_ready() && m0_start.elapsed().as_millis() < 1500 {
        let _ = runtime_driver.poll();
        embassy_time::Timer::after_millis(1).await;
    }
    if runtime_driver.is_ready() && initial_m0_firmware.kind != runtime::M0_FW_ROADRUNNER {
        runtime_driver.confirm_legacy_m0_firmware();
    }

    usb::init_event_queues();
    usb::init();

    let app_host = AppHost::new(AppId::Boot);
    let app_host = APP_HOST.init(embassy_sync::mutex::Mutex::new(app_host));

    app_host.lock().await.init();

    let mut last_m0_poll = embassy_time::Instant::now();
    let mut ticker = embassy_time::Ticker::every(embassy_time::Duration::from_millis(1));

    loop {
        ticker.next().await;

        if let Some(event) = usb::dequeue_midi_event() {
            let mut app_host_guard = app_host.lock().await;
            app_host_guard.route_midi_event(event);
            while let Some(event) = usb::dequeue_midi_event() {
                app_host_guard.route_midi_event(event);
            }
        }

        runtime_driver.leds_task();

        let m0_event = {
            let now = embassy_time::Instant::now();
            if now.duration_since(last_m0_poll).as_millis() >= 1 {
                last_m0_poll = now;
                runtime_driver.poll()
            } else {
                None
            }
        };

        let mut app_host_guard = app_host.lock().await;
        app_host_guard.route_tick_event();
        if let Some(event) = m0_event {
            app_host_guard.route_surface_event(event);
        }
        drop(app_host_guard);
        runtime_driver.leds_task();

        if let Some(message) = usb::dequeue_sysex_message() {
            let mut app_host_guard = app_host.lock().await;
            app_host_guard.receive_sysex(message.port, &message.data[..message.len]);
            while let Some(message) = usb::dequeue_sysex_message() {
                app_host_guard.receive_sysex(message.port, &message.data[..message.len]);
            }
        }

        runtime_driver.leds_task();
    }
}

fn enable_cpu_caches() {
    unsafe {
        let mut peripherals = cortex_m::Peripherals::steal();
        peripherals.SCB.enable_icache();
        peripherals.SCB.enable_dcache(&mut peripherals.CPUID);
    }
}
