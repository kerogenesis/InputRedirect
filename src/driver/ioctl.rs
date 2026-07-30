//! The wire format of the four requests the driver understands.
//!
//! The payload layouts are fixed by the driver, so the structures are
//! `repr(C, packed)` and their offsets are checked at compile time - a wrong
//! offset here would be answered by the kernel with an unhelpful invalid
//! parameter and nothing else.

use std::mem::{offset_of, size_of};

use bytemuck::{Pod, Zeroable};
use windows::Wdk::System::IO::NtDeviceIoControlFile;
use windows::Win32::Foundation::{HANDLE, NTSTATUS};
use windows::Win32::System::IO::IO_STATUS_BLOCK;

use crate::error::{Error, Result};

pub const PLUG_DEVICE: u32 = 0x002A_2000;
pub const UNPLUG_DEVICE: u32 = 0x002A_2004;
pub const SEND_KEYBOARD: u32 = 0x002A_200C;
pub const SEND_MOUSE: u32 = 0x002A_2010;

pub const VENDOR_ID: u16 = 0x046D;
pub const KEYBOARD_PRODUCT_ID: u16 = 0xC232;
pub const MOUSE_PRODUCT_ID: u16 = 0xC231;

/// The hardware id the driver matches on, in the longest form it can take: both
/// ids are always written as four hex digits, so every device's id is this wide.
const HARDWARE_ID_TEMPLATE: &str = r"LGHUBDevice\VID_0000&PID_0000";

/// The same as UTF-16, with the two trailing zero units the driver reads as the
/// end of a double null terminated string.
const HARDWARE_ID_SIZE: usize = HARDWARE_ID_TEMPLATE.len() * 2 + 4;

/// What the driver expects at the front of a plug request. The report
/// descriptor follows immediately after it.
///
/// `Pod` is what makes the layout the compiler's problem rather than a comment's:
/// it cannot be derived for a type with padding, which is exactly the property
/// that has to hold for the bytes to mean to the driver what they mean here.
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PlugRequest {
    magic: u32,
    pub device_id: u32,
    flag: u32,
    hardware_id_size: u32,
    hardware_id: [u8; 0x80],
    vendor_id: u16,
    product_id: u16,
    device_kind: u32,
    version: u16,
    _padding: u16,
    _reserved: [u8; 0xB2 - 0x9C],
    report_descriptor_size: u32,
}

const _: () = assert!(size_of::<PlugRequest>() == 0xB6);
const _: () = assert!(offset_of!(PlugRequest, hardware_id) == 0x10);
const _: () = assert!(offset_of!(PlugRequest, vendor_id) == 0x90);
const _: () = assert!(offset_of!(PlugRequest, product_id) == 0x92);
const _: () = assert!(offset_of!(PlugRequest, device_kind) == 0x94);
const _: () = assert!(offset_of!(PlugRequest, version) == 0x98);
const _: () = assert!(offset_of!(PlugRequest, report_descriptor_size) == 0xB2);
const _: () = assert!(HARDWARE_ID_SIZE <= 0x80);

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct UnplugRequest {
    size: u32,
    device_id: u32,
    _reserved: [u32; 3],
}

impl UnplugRequest {
    #[must_use]
    pub fn new(device_id: u32) -> Self {
        Self {
            size: size_of::<Self>() as u32,
            device_id,
            _reserved: [0; 3],
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> Vec<u8> {
        bytemuck::bytes_of(self).to_vec()
    }
}

/// Which of the two devices a plug request describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
}

impl DeviceKind {
    const fn code(self) -> u32 {
        match self {
            Self::Keyboard => 0,
            Self::Mouse => 1,
        }
    }

    const fn product_id(self) -> u16 {
        match self {
            Self::Keyboard => KEYBOARD_PRODUCT_ID,
            Self::Mouse => MOUSE_PRODUCT_ID,
        }
    }

    const fn report_descriptor(self) -> &'static [u8] {
        match self {
            Self::Keyboard => &KEYBOARD_REPORT_DESCRIPTOR,
            Self::Mouse => &MOUSE_REPORT_DESCRIPTOR,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Mouse => "mouse",
        }
    }
}

/// Builds the full plug payload: the header followed by the report descriptor.
#[must_use]
pub fn plug_payload(kind: DeviceKind) -> Vec<u8> {
    let product_id = kind.product_id();
    let descriptor = kind.report_descriptor();

    // The driver matches on this hardware id when it creates the child device.
    // Its length is the one HARDWARE_ID_TEMPLATE stands for.
    let hardware_id: Vec<u16> = format!("LGHUBDevice\\VID_{VENDOR_ID:04X}&PID_{product_id:04X}")
        .encode_utf16()
        .collect();
    let hardware_id_bytes: Vec<u8> = hardware_id
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    debug_assert_eq!(hardware_id_bytes.len() + 4, HARDWARE_ID_SIZE);

    let mut header = PlugRequest {
        magic: 0xB7,
        flag: 1,
        // Two trailing zero units: the driver reads the field as a
        // double null terminated string.
        hardware_id_size: (hardware_id_bytes.len() + 4) as u32,
        vendor_id: VENDOR_ID,
        product_id,
        device_kind: kind.code(),
        version: 0x0100,
        report_descriptor_size: descriptor.len() as u32,
        ..PlugRequest::zeroed()
    };
    header.hardware_id[..hardware_id_bytes.len()].copy_from_slice(&hardware_id_bytes);

    let mut payload = bytemuck::bytes_of(&header).to_vec();
    payload.extend_from_slice(descriptor);
    payload
}

/// Reads the device id the driver wrote back into the payload it was given.
///
/// A payload too short to hold one answers zero, which no device ever has: the
/// kernel decides how much of the buffer it fills, and a slice index here would
/// turn a short answer into a panic inside a driver call.
#[must_use]
pub fn plugged_device_id(payload: &[u8]) -> u32 {
    payload
        .get(4..8)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map_or(0, u32::from_le_bytes)
}

/// Sends one request to the driver.
pub fn send(handle: HANDLE, code: u32, payload: &mut [u8], expects_output: bool) -> Result<()> {
    let mut status_block = IO_STATUS_BLOCK::default();
    let input = payload.as_mut_ptr().cast();
    let length = payload.len() as u32;

    // SAFETY: the handle is open for the lifetime of the call and the buffer is
    // valid for the length we hand over. The driver writes back into the same
    // buffer for plug requests, which is why it is passed as output too.
    let status: NTSTATUS = unsafe {
        NtDeviceIoControlFile(
            handle,
            None,
            None,
            None,
            &mut status_block,
            code,
            Some(input),
            length,
            expects_output.then_some(input),
            if expects_output { length } else { 0 },
        )
    };

    if status.is_ok() {
        Ok(())
    } else {
        Err(Error::Device(format!(
            "the driver refused request {code:#010X} (status {:#010X})",
            status.0
        )))
    }
}

/// Keyboard report descriptor, as published by the original device: eight
/// modifier bits, five LED bits and six key slots.
#[rustfmt::skip]
pub const KEYBOARD_REPORT_DESCRIPTOR: [u8; 63] = [
    0x05, 0x01,             // usage page: generic desktop
    0x09, 0x06,             // usage: keyboard
    0xA1, 0x01,             // collection: application
    0x05, 0x07,             //   usage page: key codes
    0x19, 0xE0, 0x29, 0xE7, //   usages: left ctrl .. right gui
    0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08,
    0x81, 0x02,             //   input: eight modifier bits
    0x95, 0x01, 0x75, 0x08,
    0x81, 0x01,             //   input: reserved byte
    0x95, 0x05, 0x75, 0x01,
    0x05, 0x08,             //   usage page: LEDs
    0x19, 0x01, 0x29, 0x05,
    0x91, 0x02,             //   output: five LED bits
    0x95, 0x01, 0x75, 0x03,
    0x91, 0x01,             //   output: LED padding
    0x95, 0x06, 0x75, 0x08,
    0x15, 0x00, 0x25, 0xE7,
    0x05, 0x07,             //   usage page: key codes
    0x19, 0x00, 0x29, 0xE7,
    0x81, 0x00,             //   input: six key slots
    0xC0,                   // end collection
];

/// Mouse report descriptor: eight buttons, two relative axes and two wheels.
#[rustfmt::skip]
pub const MOUSE_REPORT_DESCRIPTOR: [u8; 71] = [
    0x05, 0x01,             // usage page: generic desktop
    0x09, 0x02,             // usage: mouse
    0xA1, 0x01,             // collection: application
    0xA1, 0x02,             //   collection: logical
    0x05, 0x09,             //     usage page: buttons
    0x19, 0x01, 0x29, 0x08, //     usages: buttons one to eight
    0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08,
    0x81, 0x02,             //     input: eight button bits
    0x05, 0x01,
    0x09, 0x01,             //     usage: pointer
    0xA1, 0x00,             //     collection: physical
    0x15, 0x81, 0x25, 0x7F,
    0x75, 0x08, 0x95, 0x02,
    0x09, 0x30, 0x09, 0x31,
    0x81, 0x06,             //       input: relative x and y
    0xC0,
    0x09, 0x38,             //     usage: wheel
    0x95, 0x01, 0x81, 0x06,
    0x05, 0x0C,             //     usage page: consumer
    0x09, 0x01,
    0xA1, 0x01,
    0x15, 0x81, 0x25, 0x7F,
    0x0A, 0x38, 0x02,       //       usage: horizontal pan
    0x95, 0x01, 0x81, 0x06,
    0xC0,
    0xC0,
    0xC0,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_descriptors_keep_the_length_the_driver_was_built_around() {
        assert_eq!(KEYBOARD_REPORT_DESCRIPTOR.len(), 63);
        assert_eq!(MOUSE_REPORT_DESCRIPTOR.len(), 71);
    }

    #[test]
    fn every_report_descriptor_collection_is_closed() {
        for descriptor in [
            &KEYBOARD_REPORT_DESCRIPTOR[..],
            &MOUSE_REPORT_DESCRIPTOR[..],
        ] {
            let opened = descriptor.windows(2).filter(|pair| pair[0] == 0xA1).count();
            let closed = descriptor.iter().filter(|byte| **byte == 0xC0).count();
            assert_eq!(opened, closed);
        }
    }

    #[test]
    fn the_plug_payload_is_the_header_followed_by_the_descriptor() {
        let payload = plug_payload(DeviceKind::Keyboard);

        assert_eq!(payload.len(), 0xB6 + KEYBOARD_REPORT_DESCRIPTOR.len());
        assert_eq!(&payload[0xB6..], &KEYBOARD_REPORT_DESCRIPTOR[..]);
    }

    #[test]
    fn the_plug_header_carries_the_values_the_driver_checks() {
        let payload = plug_payload(DeviceKind::Mouse);

        assert_eq!(
            u32::from_le_bytes(payload[0x00..0x04].try_into().unwrap()),
            0xB7
        );
        assert_eq!(
            u32::from_le_bytes(payload[0x08..0x0C].try_into().unwrap()),
            1
        );
        assert_eq!(
            u16::from_le_bytes(payload[0x90..0x92].try_into().unwrap()),
            VENDOR_ID
        );
        assert_eq!(
            u16::from_le_bytes(payload[0x92..0x94].try_into().unwrap()),
            MOUSE_PRODUCT_ID
        );
        assert_eq!(
            u32::from_le_bytes(payload[0x94..0x98].try_into().unwrap()),
            1
        );
        assert_eq!(
            u32::from_le_bytes(payload[0xB2..0xB6].try_into().unwrap()) as usize,
            MOUSE_REPORT_DESCRIPTOR.len()
        );
    }

    #[test]
    fn the_hardware_id_is_a_double_null_terminated_wide_string() {
        let payload = plug_payload(DeviceKind::Keyboard);
        let declared = u32::from_le_bytes(payload[0x0C..0x10].try_into().unwrap()) as usize;

        let units: Vec<u16> = payload[0x10..0x10 + declared - 4]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();

        assert_eq!(
            String::from_utf16(&units).unwrap(),
            "LGHUBDevice\\VID_046D&PID_C232"
        );
        assert_eq!(
            &payload[0x10 + declared - 4..0x10 + declared],
            &[0, 0, 0, 0]
        );
    }

    /// The field is 128 bytes and the id is built at run time, so the id of
    /// every device has to be the length the compile time assertion is about.
    #[test]
    fn the_hardware_id_of_both_devices_is_the_length_that_was_checked() {
        for kind in [DeviceKind::Keyboard, DeviceKind::Mouse] {
            let payload = plug_payload(kind);
            let declared = u32::from_le_bytes(payload[0x0C..0x10].try_into().unwrap()) as usize;

            assert_eq!(declared, HARDWARE_ID_SIZE, "{} id", kind.label());
        }
    }

    #[test]
    fn the_two_devices_do_not_claim_the_same_product_id() {
        assert_ne!(
            DeviceKind::Keyboard.product_id(),
            DeviceKind::Mouse.product_id()
        );
    }

    #[test]
    fn an_unplug_request_is_twenty_bytes_that_start_with_their_own_size() {
        let request = UnplugRequest::new(7).as_bytes();

        assert_eq!(request.len(), 20);
        assert_eq!(u32::from_le_bytes(request[0..4].try_into().unwrap()), 20);
        assert_eq!(u32::from_le_bytes(request[4..8].try_into().unwrap()), 7);
    }

    #[test]
    fn the_device_id_is_read_back_from_where_the_driver_writes_it() {
        let mut payload = plug_payload(DeviceKind::Mouse);
        payload[4..8].copy_from_slice(&42u32.to_le_bytes());

        assert_eq!(plugged_device_id(&payload), 42);
    }

    #[test]
    fn a_payload_too_short_to_hold_a_device_id_answers_zero_instead_of_panicking() {
        assert_eq!(plugged_device_id(&[]), 0);
        assert_eq!(plugged_device_id(&[1, 2, 3, 4, 5]), 0);
    }
}
