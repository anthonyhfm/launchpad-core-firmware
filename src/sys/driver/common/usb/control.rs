// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use super::UsbDeviceConfig;
use super::descriptors::{
    CONFIG_DESCRIPTOR_LEN, DESC_CONFIGURATION, DESC_DEVICE, DESC_STRING, STRING_LANG_ID,
    build_config_descriptor, build_device_descriptor, encode_string_descriptor,
};
use core::cell::UnsafeCell;

pub const REQ_GET_STATUS: u8 = 0x00;
pub const REQ_CLEAR_FEATURE: u8 = 0x01;
pub const REQ_SET_ADDRESS: u8 = 0x05;
pub const REQ_GET_DESCRIPTOR: u8 = 0x06;
pub const REQ_GET_CONFIGURATION: u8 = 0x08;
pub const REQ_SET_CONFIGURATION: u8 = 0x09;
pub const REQ_GET_INTERFACE: u8 = 0x0a;
pub const REQ_SET_INTERFACE: u8 = 0x0b;

pub struct ControlState {
    pub cfg: Option<&'static UsbDeviceConfig>,
    pub configuration: u8,
    pub pending_address: Option<u8>,
    pub tx_data: *const u8,
    pub tx_len: usize,
    pub tx_pos: usize,
    pub tx_zlp: bool,
    pub control_buf: [u8; 64],
    pub device_desc: [u8; 18],
    pub config_desc: [u8; CONFIG_DESCRIPTOR_LEN],
    pub inited: bool,
}

impl ControlState {
    pub const fn new() -> Self {
        Self {
            cfg: None,
            configuration: 0,
            pending_address: None,
            tx_data: core::ptr::null(),
            tx_len: 0,
            tx_pos: 0,
            tx_zlp: false,
            control_buf: [0; 64],
            device_desc: [0; 18],
            config_desc: [0; CONFIG_DESCRIPTOR_LEN],
            inited: false,
        }
    }

    pub fn abort_transfer(&mut self) {
        self.pending_address = None;
        self.tx_data = core::ptr::null();
        self.tx_len = 0;
        self.tx_pos = 0;
        self.tx_zlp = false;
    }

    pub fn init(&mut self, cfg: &'static UsbDeviceConfig) {
        self.cfg = Some(cfg);
        build_device_descriptor(cfg, &mut self.device_desc);
        build_config_descriptor(cfg, &mut self.config_desc);
        self.inited = true;
        self.configuration = 0;
        self.abort_transfer();
    }
}

pub struct ControlStateCell(UnsafeCell<ControlState>);
unsafe impl Sync for ControlStateCell {}

pub static CONTROL: ControlStateCell = ControlStateCell(UnsafeCell::new(ControlState::new()));

#[inline(always)]
pub fn control() -> &'static mut ControlState {
    unsafe { &mut *CONTROL.0.get() }
}

pub enum SetupAction {
    SendPacket { data: *const u8, len: usize },
    StatusIn,
    Stall,
    ConfigurationChanged(u8),
}

pub fn handle_setup_request(setup: [u8; 8]) -> SetupAction {
    handle_setup(control(), setup)
}

pub fn handle_setup(state: &mut ControlState, setup: [u8; 8]) -> SetupAction {
    state.abort_transfer();

    let Some(cfg) = state.cfg else {
        return SetupAction::Stall;
    };

    let bm_request_type = setup[0];
    let request = setup[1];
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;

    if bm_request_type & 0x60 != 0 {
        return SetupAction::Stall;
    }

    let is_device_to_host = (bm_request_type & 0x80) != 0;

    match (is_device_to_host, request) {
        (true, REQ_GET_DESCRIPTOR) => {
            let desc_type = (value >> 8) as u8;
            let desc_index = (value & 0xff) as u8;

            let (ptr, full_len) = match desc_type {
                DESC_DEVICE if desc_index == 0 => {
                    (state.device_desc.as_ptr(), state.device_desc.len())
                }
                DESC_CONFIGURATION if desc_index == 0 => {
                    (state.config_desc.as_ptr(), state.config_desc.len())
                }
                DESC_STRING => {
                    let len = match desc_index {
                        0 => {
                            state.control_buf[..4].copy_from_slice(&STRING_LANG_ID);
                            4
                        }
                        1 => encode_string_descriptor(cfg.manufacturer, &mut state.control_buf),
                        2 => encode_string_descriptor(cfg.product, &mut state.control_buf),
                        3 => encode_string_descriptor(cfg.serial_number, &mut state.control_buf),
                        4 => encode_string_descriptor(cfg.port1_name, &mut state.control_buf),
                        5 => encode_string_descriptor(cfg.port2_name, &mut state.control_buf),
                        _ => return SetupAction::Stall,
                    };
                    (state.control_buf.as_ptr(), len)
                }
                _ => return SetupAction::Stall,
            };

            start_control_read(state, ptr, full_len, length)
        }
        (false, REQ_SET_ADDRESS)
            if bm_request_type == 0
                && value <= 127
                && index == 0
                && length == 0
                && state.configuration == 0 =>
        {
            state.pending_address = Some(value as u8);
            SetupAction::StatusIn
        }
        (false, REQ_SET_CONFIGURATION) if value <= 1 => {
            state.configuration = value as u8;
            SetupAction::ConfigurationChanged(state.configuration)
        }
        (true, REQ_GET_CONFIGURATION) => {
            state.control_buf[0] = state.configuration;
            start_control_read(state, state.control_buf.as_ptr(), 1, length)
        }
        (true, REQ_GET_STATUS) => {
            state.control_buf[0] = 0;
            state.control_buf[1] = 0;
            start_control_read(state, state.control_buf.as_ptr(), 2, length)
        }
        (false, REQ_CLEAR_FEATURE) => SetupAction::StatusIn,
        (true, REQ_GET_INTERFACE) if index <= 1 => {
            state.control_buf[0] = 0;
            start_control_read(state, state.control_buf.as_ptr(), 1, length)
        }
        (false, REQ_SET_INTERFACE) if index <= 1 && value == 0 => SetupAction::StatusIn,
        _ => SetupAction::Stall,
    }
}

pub fn start_control_read(
    state: &mut ControlState,
    data: *const u8,
    data_len: usize,
    requested_len: usize,
) -> SetupAction {
    let ep0_max = state
        .cfg
        .map(|c| c.ep0_max_packet_size as usize)
        .unwrap_or(64);
    state.tx_data = data;
    state.tx_len = data_len.min(requested_len);
    state.tx_pos = 0;
    state.tx_zlp = state.tx_len != 0 && state.tx_len % ep0_max == 0 && requested_len > state.tx_len;

    let len = state.tx_len.min(ep0_max);
    state.tx_pos += len;
    SetupAction::SendPacket {
        data: state.tx_data,
        len,
    }
}

pub enum Ep0InCompletion {
    Ignore,
    SetAddress(u8),
    SendPacket { data: *const u8, len: usize },
    ArmOut,
}

pub fn complete_ep0_in(state: &mut ControlState, setup_pending: bool) -> Ep0InCompletion {
    if setup_pending {
        return Ep0InCompletion::Ignore;
    }
    if let Some(address) = state.pending_address.take() {
        Ep0InCompletion::SetAddress(address)
    } else if let Some((data, len)) = next_ep0_chunk(state) {
        Ep0InCompletion::SendPacket { data, len }
    } else {
        Ep0InCompletion::ArmOut
    }
}

pub fn next_ep0_chunk(state: &mut ControlState) -> Option<(*const u8, usize)> {
    let ep0_max = state
        .cfg
        .map(|c| c.ep0_max_packet_size as usize)
        .unwrap_or(64);
    let remain = state.tx_len.saturating_sub(state.tx_pos);
    if remain > 0 {
        let len = remain.min(ep0_max);
        let ptr = unsafe { state.tx_data.add(state.tx_pos) };
        state.tx_pos += len;
        Some((ptr, len))
    } else if state.tx_zlp {
        state.tx_zlp = false;
        Some((core::ptr::null(), 0))
    } else {
        None
    }
}
