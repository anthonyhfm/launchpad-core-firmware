// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use super::super::UsbDeviceConfig;
use super::super::control::{
    Ep0InCompletion, SetupAction, complete_ep0_in, control, handle_setup_request,
};
use super::super::midi::{SysexReceiver, parse_usb_midi_packet};
use super::super::queues::queues;
use core::cell::UnsafeCell;
use stm32_metapac as pac;

const EP_MAX_PACKET: usize = 64;
#[cfg(feature = "launchpad-pro-mk3")]
const CORE_HZ: u32 = 216_000_000;
#[cfg(not(feature = "launchpad-pro-mk3"))]
const CORE_HZ: u32 = 84_000_000;
const DISCONNECT_DELAY_MS: u32 = 10;
const MODE_SWITCH_DELAY_MS: u32 = 25;
const CORE_RESET_DELAY_US: u32 = 3;

struct OtgFsState {
    sysex_rx: SysexReceiver,
    pending_setup: Option<[u8; 8]>,
}

impl OtgFsState {
    const fn new() -> Self {
        Self {
            sysex_rx: SysexReceiver::new(),
            pending_setup: None,
        }
    }
}

struct OtgFsStateCell(UnsafeCell<OtgFsState>);
unsafe impl Sync for OtgFsStateCell {}

static OTG_STATE: OtgFsStateCell = OtgFsStateCell(UnsafeCell::new(OtgFsState::new()));

pub fn init(cfg: &'static UsbDeviceConfig) {
    cortex_m::peripheral::NVIC::mask(pac::Interrupt::OTG_FS);
    control().init(cfg);

    init_clocks_and_pins();

    let otg = pac::USB_OTG_FS;

    otg.pcgcctl().write(|w| w.0 = 0);
    otg.gahbcfg().write(|w| w.0 = 0);
    otg.dctl().modify(|w| w.set_sdis(true));
    cortex_m::asm::delay(CORE_HZ / 1_000 * DISCONNECT_DELAY_MS);

    pac::RCC.ahb2rstr().modify(|w| w.set_usb_otg_fsrst(true));
    let _ = pac::RCC.ahb2rstr().read();
    pac::RCC.ahb2rstr().modify(|w| w.set_usb_otg_fsrst(false));
    otg.pcgcctl().write(|w| w.0 = 0);

    otg.gusbcfg().write(|w| w.set_physel(true));

    while !otg.grstctl().read().ahbidl() {}

    otg.grstctl().write(|w| w.set_csrst(true));
    while otg.grstctl().read().csrst() {}
    cortex_m::asm::delay(CORE_HZ / 1_000_000 * CORE_RESET_DELAY_US);

    otg.gusbcfg().write(|w| {
        w.set_fdmod(true);
        w.set_physel(true);
        w.set_trdt(6);
    });
    cortex_m::asm::delay(CORE_HZ / 1_000 * MODE_SWITCH_DELAY_MS);
    while otg.gintsts().read().cmod() {}
    otg.dctl().write(|w| w.set_sdis(true));

    #[cfg(not(feature = "launchpad-pro-mk3"))]
    otg.gccfg_v1().write(|w| {
        w.set_pwrdwn(true);
        w.set_novbussens(true);
        w.set_vbusasen(false);
        w.set_vbusbsen(false);
        w.set_sofouten(false);
    });

    #[cfg(feature = "launchpad-pro-mk3")]
    {
        otg.gccfg_v2().write(|w| {
            w.set_pwrdwn(true);
            w.set_vbden(false);
        });
        otg.gotgctl().modify(|w| {
            w.set_bvaloen(true);
            w.set_bvaloval(true);
        });
    }

    otg.dcfg().write(|w| {
        w.set_pfivl(pac::otg::vals::Pfivl::FRAME_INTERVAL_80);
        w.set_dspd(pac::otg::vals::Dspd::FULL_SPEED_INTERNAL);
    });

    otg.grxfsiz().write(|w| w.set_rxfd(128));
    otg.dieptxf0().write(|w| {
        w.set_sa(128);
        w.set_fd(32);
    });
    otg.dieptxf(0).write(|w| {
        w.set_sa(160);
        w.set_fd(64);
    });

    otg.grstctl().write(|w| {
        w.set_rxfflsh(true);
        w.set_txfflsh(true);
        w.set_txfnum(0x10);
    });
    while otg.grstctl().read().rxfflsh() || otg.grstctl().read().txfflsh() {}

    reset_core();

    otg.diepmsk().write(|w| {
        w.set_xfrcm(true);
    });
    otg.doepmsk().write(|w| {
        w.set_xfrcm(true);
        w.set_stupm(true);
    });

    otg.daintmsk().write(|w| {
        w.set_iepm(0x03);
        w.set_oepm(0x03);
    });

    otg.gintsts()
        .write_value(pac::otg::regs::Gintsts(0xFFFF_FFFF));

    otg.gintmsk().write(|w| {
        w.set_usbrst(true);
        w.set_enumdnem(true);
        w.set_rxflvlm(true);
        w.set_iepint(true);
        w.set_oepint(true);
        w.set_usbsuspm(true);
        w.set_wuim(true);
    });

    unsafe {
        let mut nvic = cortex_m::Peripherals::steal().NVIC;
        nvic.set_priority(pac::Interrupt::OTG_FS, 0x20);
        cortex_m::peripheral::NVIC::unpend(pac::Interrupt::OTG_FS);
        otg.gahbcfg().write(|w| w.set_gint(true));
        cortex_m::peripheral::NVIC::unmask(pac::Interrupt::OTG_FS);
    }
    otg.dctl().modify(|w| w.set_sdis(false));
}

fn init_clocks_and_pins() {
    pac::RCC.ahb2enr().modify(|w| w.set_usb_otg_fsen(true));
    let _ = pac::RCC.ahb2enr().read();
    pac::RCC.ahb1enr().modify(|w| w.set_gpioaen(true));
    let _ = pac::RCC.ahb1enr().read();
    pac::GPIOA.afr(1).modify(|w| {
        w.set_afr(3, 10);
        w.set_afr(4, 10);
    });
    pac::GPIOA.ospeedr().modify(|w| {
        w.set_ospeedr(11, pac::gpio::vals::Ospeedr::VERY_HIGH_SPEED);
        w.set_ospeedr(12, pac::gpio::vals::Ospeedr::VERY_HIGH_SPEED);
    });
    pac::GPIOA.pupdr().modify(|w| {
        w.set_pupdr(11, pac::gpio::vals::Pupdr::FLOATING);
        w.set_pupdr(12, pac::gpio::vals::Pupdr::FLOATING);
    });
    pac::GPIOA.otyper().modify(|w| {
        w.set_ot(11, pac::gpio::vals::Ot::PUSH_PULL);
        w.set_ot(12, pac::gpio::vals::Ot::PUSH_PULL);
    });
    pac::GPIOA.moder().modify(|w| {
        w.set_moder(11, pac::gpio::vals::Moder::ALTERNATE);
        w.set_moder(12, pac::gpio::vals::Moder::ALTERNATE);
    });
}

pub fn poll() {
    let otg = pac::USB_OTG_FS;

    loop {
        let ints = pac::otg::regs::Gintsts(otg.gintsts().read().0 & otg.gintmsk().read().0);
        if ints.0 == 0 {
            break;
        }

        if ints.usbrst() {
            otg.gintsts().write(|w| w.set_usbrst(true));
            reset_core();
            continue;
        }

        if ints.enumdne() {
            otg.gintsts().write(|w| w.set_enumdne(true));
            otg.gusbcfg().modify(|w| w.set_trdt(6));
            otg.dctl().modify(|w| w.set_cginak(true));
            continue;
        }

        if ints.rxflvl() {
            service_rx_fifo();
        }

        if ints.iepint() {
            service_in_endpoints();
        }

        if ints.oepint() {
            service_out_endpoints();
        }

        if ints.usbsusp() {
            otg.gintsts().write(|w| w.set_usbsusp(true));
        }
        if ints.wkupint() {
            otg.gintsts().write(|w| w.set_wkupint(true));
        }

        let after_ints = otg.gintsts().read();
        if (after_ints.0 & (1 << 4 | 1 << 12 | 1 << 18 | 1 << 19)) == 0 {
            break;
        }
    }

    pump_tx();
}

fn reset_core() {
    let otg = pac::USB_OTG_FS;
    let ctrl = control();
    let Some(cfg) = ctrl.cfg else {
        return;
    };
    ctrl.init(cfg);
    queues().clear();
    let state = unsafe { &mut *OTG_STATE.0.get() };
    state.sysex_rx.reset();
    state.pending_setup = None;

    otg.dcfg().modify(|w| w.set_dad(0));
    otg.diepctl(1).write(|w| w.set_snak(true));
    otg.doepctl(1).write(|w| w.set_snak(true));
    for ep in 0..=1 {
        otg.diepint(ep).write(|w| w.0 = 0xffff_ffff);
        otg.doepint(ep).write(|w| w.0 = 0xffff_ffff);
    }
    otg.grstctl().write(|w| {
        w.set_txfflsh(true);
        w.set_txfnum(0x10);
    });
    while otg.grstctl().read().txfflsh() {}

    otg.diepctl(0).write(|w| {
        w.set_mpsiz(0);
        w.set_snak(true);
    });

    otg.doepctl(0).write(|w| w.set_mpsiz(0));
    arm_ep0_out();
}

fn arm_ep0_out() {
    let otg = pac::USB_OTG_FS;
    otg.doeptsiz(0).write(|w| {
        w.set_xfrsiz(EP_MAX_PACKET as u32);
        w.set_pktcnt(1);
        w.set_rxdpid_stupcnt(3);
    });
    otg.doepctl(0).modify(|w| {
        w.set_epena(true);
        w.set_cnak(true);
    });
}

fn service_rx_fifo() {
    let otg = pac::USB_OTG_FS;

    while otg.gintsts().read().rxflvl() {
        let status = otg.grxstsp().read();
        let ep_num = status.epnum() as usize;
        let bcnt = status.bcnt() as usize;
        let pktsts = status.pktstsd();

        match pktsts {
            pac::otg::vals::Pktstsd::SETUP_DATA_RX => {
                let w0 = otg.fifo(0).read().0;
                let w1 = otg.fifo(0).read().0;
                let mut setup = [0u8; 8];
                setup[0..4].copy_from_slice(&w0.to_ne_bytes());
                setup[4..8].copy_from_slice(&w1.to_le_bytes());

                control().abort_transfer();
                unsafe { &mut *OTG_STATE.0.get() }.pending_setup = Some(setup);
            }
            pac::otg::vals::Pktstsd::OUT_DATA_RX => {
                let mut buf = [0u8; EP_MAX_PACKET];
                let len = bcnt.min(EP_MAX_PACKET);
                let mut chunks = buf[..len].chunks_exact_mut(4);
                for chunk in &mut chunks {
                    let data = otg.fifo(0).read().0;
                    chunk.copy_from_slice(&data.to_ne_bytes());
                }
                let rem = chunks.into_remainder();
                if !rem.is_empty() {
                    let data = otg.fifo(0).read().0;
                    rem.copy_from_slice(&data.to_ne_bytes()[..rem.len()]);
                }

                if ep_num == 1 && control().configuration != 0 {
                    let state = unsafe { &mut *OTG_STATE.0.get() };
                    let q = queues();
                    for chunk in buf[..len].chunks_exact(4) {
                        parse_usb_midi_packet(
                            chunk,
                            &mut state.sysex_rx,
                            &mut |event| q.midi_rx.push(event),
                            &mut |msg| q.sysex_rx.push(msg),
                        );
                    }
                }
            }
            pac::otg::vals::Pktstsd::OUT_DATA_DONE => {
                if ep_num == 1 {
                    resume_rx();
                }
            }
            pac::otg::vals::Pktstsd::SETUP_DATA_DONE => {}
            _ => {
                let words = (bcnt + 3) / 4;
                for _ in 0..words {
                    let _ = otg.fifo(0).read().0;
                }
            }
        }
    }
}

fn resume_rx() {
    let otg = pac::USB_OTG_FS;
    if control().configuration == 0 || otg.doepctl(1).read().epena() {
        return;
    }

    otg.doeptsiz(1).write(|w| {
        w.set_pktcnt(1);
        w.set_xfrsiz(EP_MAX_PACKET as u32);
    });
    otg.doepctl(1).modify(|w| {
        w.set_cnak(true);
        w.set_epena(true);
    });
}

fn service_in_endpoints() {
    let otg = pac::USB_OTG_FS;
    let daint = otg.daint().read();

    if daint.iepint() & 1 != 0 {
        let ep_ints = otg.diepint(0).read();
        otg.diepint(0).write_value(ep_ints);

        if ep_ints.xfrc() {
            let setup_pending = unsafe { &*OTG_STATE.0.get() }.pending_setup.is_some();
            match complete_ep0_in(control(), setup_pending) {
                Ep0InCompletion::Ignore => {}
                Ep0InCompletion::SetAddress(address) => {
                    otg.dcfg().modify(|w| w.set_dad(address));
                    arm_ep0_out();
                }
                Ep0InCompletion::SendPacket { data, len } => write_ep0_in(data, len),
                Ep0InCompletion::ArmOut => arm_ep0_out(),
            }
        }
    }

    if daint.iepint() & 2 != 0 {
        let ep_ints = otg.diepint(1).read();
        otg.diepint(1).write_value(ep_ints);
        pump_tx();
    }
}

fn service_out_endpoints() {
    let otg = pac::USB_OTG_FS;
    let daint = otg.daint().read();

    if daint.oepint() & 1 != 0 {
        let ep_ints = otg.doepint(0).read();
        otg.doepint(0).write_value(ep_ints);
        if ep_ints.stup() {
            let setup = unsafe { &mut *OTG_STATE.0.get() }.pending_setup.take();
            if let Some(setup) = setup {
                stop_in_endpoint(0);
                flush_tx_fifo(0);
                otg.diepint(0).write(|w| w.0 = 0xffff_ffff);
                process_setup(setup);
            }
        } else if ep_ints.xfrc() && unsafe { &*OTG_STATE.0.get() }.pending_setup.is_none() {
            arm_ep0_out();
        }
    }
    if daint.oepint() & 2 != 0 {
        let ep_ints = otg.doepint(1).read();
        otg.doepint(1).write_value(ep_ints);
    }
}

fn process_setup(setup: [u8; 8]) {
    let otg = pac::USB_OTG_FS;

    match handle_setup_request(setup) {
        SetupAction::SendPacket { data, len } => {
            write_ep0_in(data, len);
        }
        SetupAction::StatusIn => {
            write_ep0_in(core::ptr::null(), 0);
        }
        SetupAction::Stall => {
            otg.diepctl(0).modify(|w| w.set_stall(true));
            otg.doepctl(0).modify(|w| w.set_stall(true));
        }
        SetupAction::ConfigurationChanged(cfg) => {
            if cfg != 0 {
                otg.diepctl(1).write(|w| {
                    w.set_mpsiz(64);
                    w.set_eptyp(pac::otg::vals::Eptyp::BULK);
                    w.set_txfnum(1);
                    w.set_sd0pid_sevnfrm(true);
                    w.set_snak(true);
                    w.set_usbaep(true);
                });

                otg.doeptsiz(1).write(|w| {
                    w.set_pktcnt(1);
                    w.set_xfrsiz(64);
                });
                otg.doepctl(1).write(|w| {
                    w.set_mpsiz(64);
                    w.set_eptyp(pac::otg::vals::Eptyp::BULK);
                    w.set_sd0pid_sevnfrm(true);
                    w.set_cnak(true);
                    w.set_epena(true);
                    w.set_usbaep(true);
                });

                pump_tx();
            } else {
                deconfigure_data_endpoints();
            }
            if unsafe { &*OTG_STATE.0.get() }.pending_setup.is_none() {
                write_ep0_in(core::ptr::null(), 0);
            }
        }
    }
}

fn stop_in_endpoint(ep: usize) {
    let otg = pac::USB_OTG_FS;
    if otg.diepctl(ep).read().epena() {
        otg.diepint(ep).write(|w| w.set_epdisd(true));
        otg.diepctl(ep).modify(|w| w.set_snak(true));
        otg.diepctl(ep).modify(|w| w.set_epdis(true));
        while otg.diepctl(ep).read().epena() {}
    }
}

fn flush_tx_fifo(ep: u8) {
    let otg = pac::USB_OTG_FS;
    while !otg.grstctl().read().ahbidl() {}
    otg.grstctl().write(|w| {
        w.set_txfnum(ep);
        w.set_txfflsh(true);
    });
    while otg.grstctl().read().txfflsh() {}
}

fn deconfigure_data_endpoints() {
    let otg = pac::USB_OTG_FS;
    stop_in_endpoint(1);
    if otg.doepctl(1).read().epena() {
        otg.dctl().modify(|w| w.set_sgonak(true));
        while !otg.gintsts().read().goutnakeff() {
            service_rx_fifo();
        }
        otg.doepint(1).write(|w| w.set_epdisd(true));
        if otg.doepctl(1).read().epena() {
            otg.doepctl(1).modify(|w| w.set_snak(true));
            otg.doepctl(1).modify(|w| w.set_epdis(true));
            while otg.doepctl(1).read().epena() {}
        }
        otg.dctl().modify(|w| w.set_cgonak(true));
    }
    otg.diepctl(1).write(|w| w.set_snak(true));
    otg.doepctl(1).write(|w| w.set_snak(true));
    flush_tx_fifo(1);
    service_rx_fifo();
    otg.diepint(1).write(|w| w.0 = 0xffff_ffff);
    otg.doepint(1).write(|w| w.0 = 0xffff_ffff);
    queues().clear();
    unsafe { &mut *OTG_STATE.0.get() }.sysex_rx.reset();
}

fn write_ep0_in(data: *const u8, len: usize) {
    let otg = pac::USB_OTG_FS;

    otg.dieptsiz(0).write(|w| {
        w.set_pktcnt(1);
        w.set_xfrsiz(len as _);
    });
    otg.diepctl(0).modify(|w| {
        w.set_cnak(true);
        w.set_epena(true);
    });

    if len > 0 && !data.is_null() {
        let slice = unsafe { core::slice::from_raw_parts(data, len) };
        let fifo = otg.fifo(0);
        let mut chunks = slice.chunks_exact(4);
        for chunk in &mut chunks {
            let val = u32::from_ne_bytes(chunk.try_into().unwrap());
            fifo.write_value(pac::otg::regs::Fifo(val));
        }
        let rem = chunks.remainder();
        if !rem.is_empty() {
            let mut tmp = [0u8; 4];
            tmp[..rem.len()].copy_from_slice(rem);
            let val = u32::from_ne_bytes(tmp);
            fifo.write_value(pac::otg::regs::Fifo(val));
        }
    }
}

pub fn pump_tx() {
    let otg = pac::USB_OTG_FS;
    let ctrl = control();

    if ctrl.configuration == 0 {
        return;
    }

    let diepctl = otg.diepctl(1).read();
    if !diepctl.usbaep() || diepctl.epena() {
        return;
    }

    let q = queues();
    let mut payload = [0u8; EP_MAX_PACKET];
    let mut count = 0;

    while count + 4 <= EP_MAX_PACKET {
        let Some(packet) = q.midi_tx.pop() else {
            break;
        };
        payload[count..count + 4].copy_from_slice(&packet.data);
        count += 4;
    }

    if count > 0 {
        otg.dieptsiz(1).write(|w| {
            w.set_pktcnt(1);
            w.set_xfrsiz(count as _);
        });
        otg.diepctl(1).modify(|w| {
            w.set_cnak(true);
            w.set_epena(true);
        });

        let fifo = otg.fifo(1);
        let slice = &payload[..count];
        let mut chunks = slice.chunks_exact(4);
        for chunk in &mut chunks {
            let val = u32::from_ne_bytes(chunk.try_into().unwrap());
            fifo.write_value(pac::otg::regs::Fifo(val));
        }
        let rem = chunks.remainder();
        if !rem.is_empty() {
            let mut tmp = [0u8; 4];
            tmp[..rem.len()].copy_from_slice(rem);
            let val = u32::from_ne_bytes(tmp);
            fifo.write_value(pac::otg::regs::Fifo(val));
        }
    }
}

#[unsafe(export_name = "OTG_FS")]
pub extern "C" fn otg_fs_handler() {
    poll();
}
