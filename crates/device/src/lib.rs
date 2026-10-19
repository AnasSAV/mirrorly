//! Phone connection: usbmuxd (TCP_NODELAY), CoreDeviceProxy tunnel,
//! Developer Disk Image, displayservice media session, RTCP feedback, HID
//! execution, and reconnect. Everything runs on one dedicated network thread.

pub mod clipboard;
pub mod conn;
pub mod docs;
pub mod ddi;
pub mod feedback;
pub mod hid;
pub mod media;
pub mod pairing;
pub mod paths;
pub mod phones;
pub mod session;
pub mod wifi;

pub use feedback::RctlMode;
pub use session::{
    DeviceEvent, DeviceHandle, DeviceInfo, InputSender, KeyframeRequester, MediaSink, NEEDS_27, Options, Transport, USER_DISCONNECT,
    start,
};
