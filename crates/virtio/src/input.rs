//! virtio-input — the bridge between the host keymapping engine and the
//! guest Android input subsystem.
//!
//! The guest sees a normal evdev input device. When the host's
//! `InputTranslator` produces an `OutputAction`, the virtio-input device
//! packages it as a Linux `input_event` struct and pushes it into the
//! virtqueue. The guest kernel's input subsystem consumes the events and
//! forwards them to Android's `InputDispatcher`.
//!
//! ## Event format
//!
//! Each event is a 24-byte struct:
//!
//! ```text
//! struct input_event {
//!     struct timeval time;  // 16 bytes on 64-bit, 8 on 32-bit
//!     __u16 type;
//!     __u16 code;
//!     __s32 value;
//! };
//! ```
//!
//! We use the 64-bit layout because Android-x86 is a 64-bit kernel.

use std::sync::Arc;

use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use tracing::debug;

use crate::queue::VirtQueue;
use crate::transport::DeviceId;
use crate::VirtioDevice;
use nitroid_core::Result;
use nitroid_input::OutputAction;

/// Linux evdev event types we care about.
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;

/// ABS codes used by the multi-touch screen.
pub const ABS_MT_SLOT: u16 = 0x2F;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_PRESSURE: u16 = 0x3A;

/// SYN codes.
pub const SYN_REPORT: u16 = 0;

/// A single Linux `input_event` (64-bit layout). 24 bytes total.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InputEventStruct {
    pub tv_sec: u64,
    pub tv_usec: u64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEventStruct {
    pub fn new(type_: u16, code: u16, value: i32) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            tv_sec: now.as_secs(),
            tv_usec: now.subsec_micros() as u64,
            type_,
            code,
            value,
        }
    }
}

/// virtio-input device. Receives `OutputAction`s from the host input engine
/// and translates them into evdev events the guest kernel can consume.
pub struct VirtioInput {
    /// Pending events waiting to be drained by the guest.
    pending: Arc<Mutex<Vec<InputEventStruct>>>,
    /// Receiver for actions from the host InputTranslator.
    /// Held as an Option so we can detach and re-attach at runtime.
    rx: Mutex<Option<Receiver<OutputAction>>>,
}

impl VirtioInput {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
            rx: Mutex::new(None),
        }
    }

    /// Attach a receiver for host-side actions. When the guest polls the
    /// virtqueue, the device drains any pending actions and converts them.
    pub fn attach_receiver(&self, rx: Receiver<OutputAction>) {
        *self.rx.lock() = Some(rx);
    }

    /// Inject an action directly (used by tests).
    pub fn inject(&self, action: OutputAction) {
        let events = translate_action(action);
        self.pending.lock().extend(events);
    }

    /// Drain up to `max` pending events. Returns the number drained.
    pub fn drain(&self, max: usize) -> Vec<InputEventStruct> {
        let mut pending = self.pending.lock();
        let take = pending.len().min(max);
        let out: Vec<_> = pending.drain(..take).collect();
        out
    }

    /// Poll the host-side receiver for new actions and translate them.
    pub fn poll_host(&self) {
        let rx = self.rx.lock();
        if let Some(rx) = rx.as_ref() {
            while let Ok(action) = rx.try_recv() {
                let events = translate_action(action);
                self.pending.lock().extend(events);
            }
        }
    }
}

impl Default for VirtioInput {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDevice for VirtioInput {
    fn device_id(&self) -> DeviceId {
        DeviceId::Input
    }

    fn features(&self) -> u64 {
        // VIRTIO_INPUT_F_EVENT (0) — no extra features
        0
    }

    fn num_queues(&self) -> usize {
        2 // events queue + status queue
    }

    fn process_queue(&self, _queue_idx: usize, queue: &VirtQueue) -> Result<usize> {
        self.poll_host();
        let pending_events = self.pending.lock().len();
        let queue_pending = queue.pending();
        if pending_events == 0 || queue_pending == 0 {
            return Ok(0);
        }
        // In a full implementation we'd write `pending_events` input_event
        // structs into the guest buffer referenced by the descriptor, then
        // mark the descriptor used. Without guest memory access we just
        // drain our local queue so it doesn't grow unbounded.
        let _ = self.drain(pending_events.min(queue_pending as usize));
        debug!(
            drained = pending_events,
            "virtio-input: drained pending events"
        );
        Ok(pending_events.min(queue_pending as usize))
    }

    fn reset(&self) {
        self.pending.lock().clear();
    }
}

/// Translate a host `OutputAction` into one or more evdev events.
pub fn translate_action(action: OutputAction) -> Vec<InputEventStruct> {
    let mut out = Vec::with_capacity(8);
    match action {
        OutputAction::TouchDown {
            slot,
            x,
            y,
            pressure,
        } => {
            out.push(InputEventStruct::new(EV_ABS, ABS_MT_SLOT, slot as i32));
            out.push(InputEventStruct::new(
                EV_ABS,
                ABS_MT_TRACKING_ID,
                slot as i32,
            ));
            out.push(InputEventStruct::new(EV_ABS, ABS_MT_POSITION_X, x as i32));
            out.push(InputEventStruct::new(EV_ABS, ABS_MT_POSITION_Y, y as i32));
            out.push(InputEventStruct::new(
                EV_ABS,
                ABS_MT_PRESSURE,
                pressure as i32,
            ));
            out.push(InputEventStruct::new(EV_SYN, SYN_REPORT, 0));
        }
        OutputAction::TouchMove { slot, x, y } => {
            out.push(InputEventStruct::new(EV_ABS, ABS_MT_SLOT, slot as i32));
            out.push(InputEventStruct::new(EV_ABS, ABS_MT_POSITION_X, x as i32));
            out.push(InputEventStruct::new(EV_ABS, ABS_MT_POSITION_Y, y as i32));
            out.push(InputEventStruct::new(EV_SYN, SYN_REPORT, 0));
        }
        OutputAction::TouchUp { slot } => {
            out.push(InputEventStruct::new(EV_ABS, ABS_MT_SLOT, slot as i32));
            out.push(InputEventStruct::new(EV_ABS, ABS_MT_TRACKING_ID, -1));
            out.push(InputEventStruct::new(EV_SYN, SYN_REPORT, 0));
        }
        OutputAction::KeyDown { code } => {
            out.push(InputEventStruct::new(EV_KEY, code, 1));
            out.push(InputEventStruct::new(EV_SYN, SYN_REPORT, 0));
        }
        OutputAction::KeyUp { code } => {
            out.push(InputEventStruct::new(EV_KEY, code, 0));
            out.push(InputEventStruct::new(EV_SYN, SYN_REPORT, 0));
        }
    }
    out
}

/// Convert a raw evdev-style touch event into evdev events. Used by the
/// virtualization backend's `inject_input` path when bypassing the
/// OutputAction abstraction (e.g. for hardware passthrough).
pub fn translate_touch_event(
    slot: u32,
    x: u32,
    y: u32,
    pressure: u16,
    active: bool,
) -> Vec<InputEventStruct> {
    if active {
        translate_action(OutputAction::TouchDown {
            slot,
            x,
            y,
            pressure,
        })
    } else {
        translate_action(OutputAction::TouchUp { slot })
    }
}

/// Convert a raw evdev-style key event into evdev events.
pub fn translate_key_event(code: u16, pressed: bool) -> Vec<InputEventStruct> {
    if pressed {
        translate_action(OutputAction::KeyDown { code })
    } else {
        translate_action(OutputAction::KeyUp { code })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_down_produces_full_mt_sequence() {
        let events = translate_action(OutputAction::TouchDown {
            slot: 0,
            x: 640,
            y: 360,
            pressure: 255,
        });
        // slot, tracking_id, x, y, pressure, syn = 6 events
        assert_eq!(events.len(), 6);
        assert_eq!(events[0].code, ABS_MT_SLOT);
        assert_eq!(events[5].type_, EV_SYN);
    }

    #[test]
    fn touch_up_marks_tracking_id_invalid() {
        let events = translate_action(OutputAction::TouchUp { slot: 2 });
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].code, ABS_MT_TRACKING_ID);
        assert_eq!(events[1].value, -1);
    }

    #[test]
    fn inject_and_drain() {
        let dev = VirtioInput::new();
        dev.inject(OutputAction::TouchDown {
            slot: 0,
            x: 1,
            y: 2,
            pressure: 255,
        });
        dev.inject(OutputAction::TouchUp { slot: 0 });
        let drained = dev.drain(100);
        // 6 + 3 = 9 events
        assert_eq!(drained.len(), 9);
        // After drain, queue is empty.
        let again = dev.drain(100);
        assert!(again.is_empty());
    }

    #[test]
    fn input_event_struct_is_24_bytes() {
        assert_eq!(std::mem::size_of::<InputEventStruct>(), 24);
    }
}
