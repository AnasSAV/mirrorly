//! Phone connection: usbmuxd (TCP_NODELAY), CoreDeviceProxy tunnel,
//! Developer Disk Image, displayservice media session, RTCP feedback, HID
//! execution, and reconnect. Everything runs on one dedicated network thread.

pub mod conn;
pub mod ddi;
pub mod feedback;
pub mod hid;
pub mod media;
pub mod paths;
pub mod session;
pub mod wifi;

pub use feedback::RctlMode;
pub use session::{
    DeviceEvent, DeviceHandle, DeviceInfo, InputSender, KeyframeRequester, MediaSink, Options, Transport, start,
};
