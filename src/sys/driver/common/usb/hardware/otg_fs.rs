// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use super::super::UsbDeviceConfig;
use super::super::control::{
    Ep0InCompletion, SetupAction, complete_ep0_in, control, handle_setup_request,
};
use super::super::midi::{SysexReceiver, parse_usb_midi_packet};
use super::super::queues::queues;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use stm32_metapac as pac;

#[path = "otg_fs_init.rs"]
mod initialization;
use initialization::Error as HardwareError;

const EP_MAX_PACKET: usize = 64;

static READY: AtomicBool = AtomicBool::new(false);
static LAST_ERROR: AtomicU8 = AtomicU8::new(0);

fn fail_hardware(error: HardwareError) {
    let _ = LAST_ERROR.compare_exchange(0, error as u8, Ordering::Relaxed, Ordering::Relaxed);
    READY.store(false, Ordering::Release);
    cortex_m::peripheral::NVIC::mask(pac::Interrupt::OTG_FS);
    let otg = pac::USB_OTG_FS;
    otg.gintmsk().write(|w| w.0 = 0);
    otg.gahbcfg().modify(|w| w.set_gint(false));
    otg.dctl().modify(|w| w.set_sdis(true));
}

struct OtgFsState {
    sysex_rx: SysexReceiver,
    pending_setup: Option<[u8; 8]>,
    ep0_status_in: bool,
}

impl OtgFsState {
    const fn new() -> Self {
        Self {
            sysex_rx: SysexReceiver::new(),
            pending_setup: None,
            ep0_status_in: false,
        }
    }
}

struct OtgFsStateCell(UnsafeCell<OtgFsState>);
unsafe impl Sync for OtgFsStateCell {}

static OTG_STATE: OtgFsStateCell = OtgFsStateCell(UnsafeCell::new(OtgFsState::new()));

pub fn init(cfg: &'static UsbDeviceConfig) {
    cortex_m::peripheral::NVIC::mask(pac::Interrupt::OTG_FS);
    READY.store(false, Ordering::Release);
    LAST_ERROR.store(0, Ordering::Relaxed);
    control().init(cfg);
    queues().clear();
    let state = unsafe { &mut *OTG_STATE.0.get() };
    state.sysex_rx.reset();
    state.pending_setup = None;
    state.ep0_status_in = false;

    if let Err(error) = initialization::initialize() {
        fail_hardware(error);
        return;
    }
    unsafe {
        let mut nvic = cortex_m::Peripherals::steal().NVIC;
        nvic.set_priority(pac::Interrupt::OTG_FS, 0x40);
        cortex_m::peripheral::NVIC::unpend(pac::Interrupt::OTG_FS);
    }
    READY.store(true, Ordering::Release);
    initialization::connect();
    unsafe {
        cortex_m::peripheral::NVIC::unmask(pac::Interrupt::OTG_FS);
    }
}

pub fn poll() {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    if let Err(error) = poll_inner() {
        fail_hardware(error);
    }
}

fn poll_inner() -> Result<(), HardwareError> {
    let otg = pac::USB_OTG_FS;
    let mut budget = initialization::POLL_LIMIT;

    loop {
        let ints = pac::otg::regs::Gintsts(otg.gintsts().read().0 & otg.gintmsk().read().0);
        if ints.0 == 0 {
            break;
        }
        budget = budget
            .checked_sub(1)
            .ok_or(HardwareError::InterruptProgress)?;

        if ints.usbrst() {
            reset_core()?;
            otg.gintsts().write(|w| w.set_usbrst(true));
            continue;
        }

        if ints.enumdne() {
            otg.gintsts().write(|w| w.set_enumdne(true));
            otg.diepctl(0).modify(|w| w.set_mpsiz(0));
            otg.gusbcfg().modify(|w| w.set_trdt(6));
            otg.dctl().modify(|w| w.set_cginak(true));
            continue;
        }

        if ints.rxflvl() {
            service_rx_fifo()?;
        }

        if ints.iepint() {
            service_in_endpoints();
        }

        if ints.oepint() {
            service_out_endpoints()?;
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
    Ok(())
}

fn reset_core() -> Result<(), HardwareError> {
    let otg = pac::USB_OTG_FS;
    let Some(cfg) = control().cfg else {
        return Ok(());
    };
    control().init(cfg);
    queues().clear();
    let state = unsafe { &mut *OTG_STATE.0.get() };
    state.sysex_rx.reset();
    state.pending_setup = None;
    state.ep0_status_in = false;

    otg.dctl().modify(|w| w.set_rwusig(false));
    initialization::flush_tx(0x10)?;
    for ep in 0..initialization::ENDPOINT_COUNT {
        otg.diepint(ep).write(|w| w.0 = 0xffff_ffff);
        otg.doepint(ep).write(|w| w.0 = 0xffff_ffff);
        otg.diepctl(ep).modify(|w| w.set_stall(false));
        otg.doepctl(ep).modify(|w| w.set_stall(false));
    }
    otg.daintmsk().write(|w| {
        w.set_iepm(1);
        w.set_oepm(1);
    });
    otg.doepmsk().write(|w| {
        w.set_stupm(true);
        w.set_xfrcm(true);
    });
    otg.diepmsk().write(|w| w.set_xfrcm(true));
    otg.dcfg().modify(|w| w.set_dad(0));
    arm_ep0_setup();
    Ok(())
}

fn arm_ep0_setup() {
    let otg = pac::USB_OTG_FS;
    // SETUP reception reserves three 8-byte packets.
    otg.doeptsiz(0).write(|w| {
        w.set_xfrsiz(24);
        w.set_pktcnt(1);
        w.set_rxdpid_stupcnt(3);
    });
    otg.doepctl(0).modify(|w| {
        w.set_epena(true);
        w.set_usbaep(true);
    });
}

fn arm_ep0_status_out() {
    let otg = pac::USB_OTG_FS;
    // The status OUT transfer reserves one full packet despite its zero length.
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

fn service_rx_fifo() -> Result<(), HardwareError> {
    let otg = pac::USB_OTG_FS;
    let mut budget = initialization::POLL_LIMIT;

    while otg.gintsts().read().rxflvl() {
        budget = budget.checked_sub(1).ok_or(HardwareError::RxDrain)?;
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
                let state = unsafe { &mut *OTG_STATE.0.get() };
                state.pending_setup = Some(setup);
                state.ep0_status_in = false;
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
    Ok(())
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
                    arm_ep0_setup();
                }
                Ep0InCompletion::SendPacket { data, len } => write_ep0_in(data, len),
                Ep0InCompletion::ArmOut => {
                    if unsafe { &*OTG_STATE.0.get() }.ep0_status_in {
                        arm_ep0_setup();
                    } else {
                        arm_ep0_status_out();
                    }
                }
            }
        }
    }

    if daint.iepint() & 2 != 0 {
        let ep_ints = otg.diepint(1).read();
        otg.diepint(1).write_value(ep_ints);
        pump_tx();
    }
}

fn service_out_endpoints() -> Result<(), HardwareError> {
    let otg = pac::USB_OTG_FS;
    let daint = otg.daint().read();

    if daint.oepint() & 1 != 0 {
        let ep_ints = otg.doepint(0).read();
        otg.doepint(0).write_value(ep_ints);
        if ep_ints.stup() {
            let setup = unsafe { &mut *OTG_STATE.0.get() }.pending_setup.take();
            if let Some(setup) = setup {
                otg.diepint(0).write(|w| w.0 = 0xffff_ffff);
                process_setup(setup)?;
            }
        } else if ep_ints.xfrc() && unsafe { &*OTG_STATE.0.get() }.pending_setup.is_none() {
            arm_ep0_setup();
        }
    }
    if daint.oepint() & 2 != 0 {
        let ep_ints = otg.doepint(1).read();
        otg.doepint(1).write_value(ep_ints);
    }
    Ok(())
}

fn process_setup(setup: [u8; 8]) -> Result<(), HardwareError> {
    let otg = pac::USB_OTG_FS;

    match handle_setup_request(setup) {
        SetupAction::SendPacket { data, len } => {
            unsafe { &mut *OTG_STATE.0.get() }.ep0_status_in = false;
            write_ep0_in(data, len);
        }
        SetupAction::StatusIn => {
            unsafe { &mut *OTG_STATE.0.get() }.ep0_status_in = true;
            if let Some(address) = control().pending_address.take() {
                otg.dcfg().modify(|w| w.set_dad(address));
            }
            write_ep0_in(core::ptr::null(), 0);
        }
        SetupAction::Stall => {
            otg.diepctl(0).modify(|w| w.set_stall(true));
            otg.doepctl(0).modify(|w| w.set_stall(true));
            arm_ep0_setup();
        }
        SetupAction::ConfigurationChanged(cfg) => {
            unsafe { &mut *OTG_STATE.0.get() }.ep0_status_in = true;
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

                otg.daintmsk().write(|w| {
                    w.set_iepm(3);
                    w.set_oepm(3);
                });
                pump_tx();
            } else {
                deconfigure_data_endpoints()?;
                otg.daintmsk().write(|w| {
                    w.set_iepm(1);
                    w.set_oepm(1);
                });
            }
            if unsafe { &*OTG_STATE.0.get() }.pending_setup.is_none() {
                write_ep0_in(core::ptr::null(), 0);
            }
        }
    }
    Ok(())
}

fn stop_in_endpoint(ep: usize) -> Result<(), HardwareError> {
    let otg = pac::USB_OTG_FS;
    if otg.diepctl(ep).read().epena() {
        otg.diepint(ep).write(|w| w.set_epdisd(true));
        otg.diepctl(ep).modify(|w| w.set_snak(true));
        otg.diepctl(ep).modify(|w| w.set_epdis(true));
        initialization::wait_until(HardwareError::InDisable, || !otg.diepctl(ep).read().epena())?;
    }
    Ok(())
}

fn deconfigure_data_endpoints() -> Result<(), HardwareError> {
    let otg = pac::USB_OTG_FS;
    stop_in_endpoint(1)?;
    if otg.doepctl(1).read().epena() {
        otg.dctl().modify(|w| w.set_sgonak(true));
        let mut budget = initialization::POLL_LIMIT;
        while !otg.gintsts().read().goutnakeff() {
            budget = budget.checked_sub(1).ok_or(HardwareError::OutNak)?;
            service_rx_fifo()?;
        }
        otg.doepint(1).write(|w| w.set_epdisd(true));
        if otg.doepctl(1).read().epena() {
            otg.doepctl(1).modify(|w| w.set_snak(true));
            otg.doepctl(1).modify(|w| w.set_epdis(true));
            initialization::wait_until(HardwareError::OutDisable, || {
                !otg.doepctl(1).read().epena()
            })?;
        }
        otg.dctl().modify(|w| w.set_cgonak(true));
    }
    otg.diepctl(1).write(|w| w.set_snak(true));
    otg.doepctl(1).write(|w| w.set_snak(true));
    initialization::flush_tx(1)?;
    service_rx_fifo()?;
    otg.diepint(1).write(|w| w.0 = 0xffff_ffff);
    otg.doepint(1).write(|w| w.0 = 0xffff_ffff);
    queues().clear();
    unsafe { &mut *OTG_STATE.0.get() }.sysex_rx.reset();
    Ok(())
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
    if !READY.load(Ordering::Acquire) {
        return;
    }
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
