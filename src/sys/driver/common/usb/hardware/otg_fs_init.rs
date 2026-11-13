// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister

//! OTG initialization matching Pro MK3 465, X 351 and Mini MK3 407.

use stm32_metapac as pac;

pub(super) const POLL_LIMIT: u32 = 200_000;
const CORE_HZ: u32 = if cfg!(feature = "launchpad-pro-mk3") {
    216_000_000
} else {
    84_000_000
};
// F401 endpoint registers 4 and 5 are reserved (ST AN4879).
pub(super) const ENDPOINT_COUNT: usize = if cfg!(feature = "launchpad-pro-mk3") {
    6
} else {
    4
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Error {
    AhbIdle = 1,
    CoreReset,
    DeviceMode,
    TxFlush,
    RxFlush,
    InDisable,
    OutNak,
    OutDisable,
    RxDrain,
    InterruptProgress,
}

pub(super) fn wait_until(error: Error, mut ready: impl FnMut() -> bool) -> Result<(), Error> {
    // IRQ waits cannot depend on a lower-priority timer ticking.
    for _ in 0..POLL_LIMIT {
        if ready() {
            return Ok(());
        }
    }
    Err(error)
}

fn delay_ms(ms: u32) {
    cortex_m::asm::delay(CORE_HZ / 1_000 * ms);
}

fn init_pins_and_clocks() {
    pac::RCC.ahb1enr().modify(|w| w.set_gpioaen(true));
    let _ = pac::RCC.ahb1enr().read();
    let gpio = pac::GPIOA;
    gpio.afr(1).modify(|w| {
        w.set_afr(3, 10);
        w.set_afr(4, 10);
    });
    gpio.ospeedr().modify(|w| {
        w.set_ospeedr(11, pac::gpio::vals::Ospeedr::VERY_HIGH_SPEED);
        w.set_ospeedr(12, pac::gpio::vals::Ospeedr::VERY_HIGH_SPEED);
    });
    gpio.pupdr().modify(|w| {
        w.set_pupdr(11, pac::gpio::vals::Pupdr::FLOATING);
        w.set_pupdr(12, pac::gpio::vals::Pupdr::FLOATING);
    });
    gpio.otyper().modify(|w| {
        w.set_ot(11, pac::gpio::vals::Ot::PUSH_PULL);
        w.set_ot(12, pac::gpio::vals::Ot::PUSH_PULL);
    });
    gpio.moder().modify(|w| {
        w.set_moder(11, pac::gpio::vals::Moder::ALTERNATE);
        w.set_moder(12, pac::gpio::vals::Moder::ALTERNATE);
    });

    // Only the Pro MK3 configures the OTG ID pin.
    #[cfg(feature = "launchpad-pro-mk3")]
    {
        gpio.afr(1).modify(|w| w.set_afr(2, 10));
        gpio.ospeedr()
            .modify(|w| w.set_ospeedr(10, pac::gpio::vals::Ospeedr::VERY_HIGH_SPEED));
        gpio.otyper()
            .modify(|w| w.set_ot(10, pac::gpio::vals::Ot::OPEN_DRAIN));
        gpio.pupdr()
            .modify(|w| w.set_pupdr(10, pac::gpio::vals::Pupdr::PULL_UP));
        gpio.moder()
            .modify(|w| w.set_moder(10, pac::gpio::vals::Moder::ALTERNATE));
    }

    pac::RCC.ahb2enr().modify(|w| w.set_usb_otg_fsen(true));
    let _ = pac::RCC.ahb2enr().read();
    pac::RCC.apb2enr().modify(|w| w.set_syscfgen(true));
    let _ = pac::RCC.apb2enr().read();
    // Stock MSP does not pulse RCC's USB peripheral reset here.
}

#[inline(never)]
fn init_core() -> Result<(), Error> {
    let otg = pac::USB_OTG_FS;
    // Stock ORs PHYSEL; preserve mode and turnaround fields across handoff.
    otg.gusbcfg().modify(|w| w.set_physel(true));
    wait_until(Error::AhbIdle, || otg.grstctl().read().ahbidl())?;
    otg.grstctl().modify(|w| w.set_csrst(true));
    wait_until(Error::CoreReset, || !otg.grstctl().read().csrst())?;
    // Power the PHY after core reset, before forcing device mode.
    otg.gccfg_v1().write(|w| w.set_pwrdwn(true));
    Ok(())
}

pub(super) fn flush_tx(fifo: u8) -> Result<(), Error> {
    let otg = pac::USB_OTG_FS;
    otg.grstctl().write(|w| {
        w.set_txfflsh(true);
        w.set_txfnum(fifo);
    });
    wait_until(Error::TxFlush, || !otg.grstctl().read().txfflsh())
}

fn flush_rx() -> Result<(), Error> {
    let otg = pac::USB_OTG_FS;
    otg.grstctl().write(|w| w.set_rxfflsh(true));
    wait_until(Error::RxFlush, || !otg.grstctl().read().rxfflsh())
}

fn init_device() -> Result<(), Error> {
    let otg = pac::USB_OTG_FS;
    #[cfg(feature = "launchpad-pro-mk3")]
    {
        // Stock toggles the VBUS detector before overriding the session.
        otg.gccfg_v2().modify(|w| w.set_vbden(true));
        otg.gccfg_v2().modify(|w| w.set_vbden(false));
        otg.gotgctl().modify(|w| w.set_bvaloen(true));
        otg.gotgctl().modify(|w| w.set_bvaloval(true));
    }
    #[cfg(not(feature = "launchpad-pro-mk3"))]
    {
        otg.gccfg_v1().modify(|w| w.set_novbussens(true));
    }
    // F401 GOTGCTL bits 2..7 are reserved; NOVBUSSENS is its stock path.
    otg.pcgcctl().write(|w| w.0 = 0);
    otg.dcfg().modify(|w| {
        w.set_pfivl(pac::otg::vals::Pfivl::FRAME_INTERVAL_80);
        w.set_dspd(pac::otg::vals::Dspd::FULL_SPEED_INTERNAL);
    });

    flush_tx(0x10)?;
    flush_rx()?;
    otg.diepmsk().write(|w| w.0 = 0);
    otg.doepmsk().write(|w| w.0 = 0);
    // DAINT is a read-only summary; endpoint interrupt acknowledgements clear it.
    otg.daintmsk().write(|w| w.0 = 0);
    for ep in 0..ENDPOINT_COUNT {
        let in_enabled = otg.diepctl(ep).read().epena();
        otg.diepctl(ep).write(|w| {
            if in_enabled {
                w.set_epdis(true);
                w.set_snak(true);
            }
        });
        otg.dieptsiz(ep).write(|w| w.0 = 0);
        otg.diepint(ep).write(|w| w.0 = 0xff);
        let out_enabled = otg.doepctl(ep).read().epena();
        otg.doepctl(ep).write(|w| {
            if out_enabled {
                w.set_epdis(true);
                w.set_snak(true);
            }
        });
        otg.doeptsiz(ep).write(|w| w.0 = 0);
        otg.doepint(ep).write(|w| w.0 = 0xff);
    }

    otg.gintmsk().write(|w| w.0 = 0);
    otg.gintsts().write(|w| w.0 = 0xbfff_ffff);
    // Enable only sources serviced by the interrupt handler.
    otg.gintmsk().write(|w| {
        w.set_rxflvlm(true);
        w.set_usbsuspm(true);
        w.set_usbrst(true);
        w.set_enumdnem(true);
        w.set_iepint(true);
        w.set_oepint(true);
        w.set_wuim(true);
    });
    Ok(())
}

pub(super) fn initialize() -> Result<(), Error> {
    init_pins_and_clocks();
    let otg = pac::USB_OTG_FS;
    otg.gahbcfg().modify(|w| w.set_gint(false));
    init_core()?;
    otg.gusbcfg().modify(|w| {
        w.set_fhmod(false);
        w.set_fdmod(false);
    });
    otg.gusbcfg().modify(|w| w.set_fdmod(true));
    delay_ms(50);
    if otg.gintsts().read().cmod() {
        return Err(Error::DeviceMode);
    }
    init_device()?;

    otg.dctl().modify(|w| w.set_sdis(true));
    delay_ms(3);
    otg.grxfsiz().write(|w| w.set_rxfd(128));
    otg.dieptxf0().write(|w| {
        w.set_sa(128);
        w.set_fd(48);
    });
    for fifo in 0..3 {
        otg.dieptxf(fifo).write(|w| {
            w.set_sa(176 + fifo as u16 * 48);
            w.set_fd(48);
        });
    }
    Ok(())
}

pub(super) fn connect() {
    let otg = pac::USB_OTG_FS;
    otg.dctl().modify(|w| w.set_sdis(false));
    delay_ms(3);
    otg.gahbcfg().modify(|w| w.set_gint(true));
}
