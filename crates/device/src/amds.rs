//! Who answers on 127.0.0.1:27015. Mirrorly trusts that port as Apple
//! Mobile Device Service: the device list, pairing records and every stream
//! to a USB device go through it. Loopback is shared by every account on
//! the PC, so when Apple's service isn't holding the port, another user's
//! program could, and would become "the iPhone" (receiving the clipboard and
//! keystrokes). So each connection is checked: the process listening on the
//! port must be the running service of that name, which only an
//! administrator can install.

use std::io;

use windows::{
    Win32::{
        NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER},
        Networking::WinSock::AF_INET,
        System::Services::{
            CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE, SC_MANAGER_CONNECT,
            SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS_PROCESS,
        },
    },
    core::w,
};

pub const PORT: u16 = 27015;

/// Fails unless Apple Mobile Device Service is what listens on the port.
pub fn check_listener() -> io::Result<()> {
    let Some(service) = service_pid() else {
        return Err(io::Error::new(io::ErrorKind::NotFound, "Apple Mobile Device Service is not running"));
    };
    match listener_pid(PORT)? {
        Some(pid) if pid == service => Ok(()),
        Some(pid) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("127.0.0.1:{PORT} is held by process {pid}, not Apple Mobile Device Service ({service})"),
        )),
        None => Err(io::Error::new(io::ErrorKind::NotFound, format!("nothing listens on 127.0.0.1:{PORT}"))),
    }
}

/// The process that a connection to 127.0.0.1:`port` reaches: the one
/// bound to 127.0.0.1, else one bound to all addresses.
fn listener_pid(port: u16) -> io::Result<Option<u32>> {
    let rows = tcp_listeners()?;
    let on = |addr: [u8; 4]| rows.iter().find(|r| r.0 == addr && r.1 == port).map(|r| r.2);
    Ok(on([127, 0, 0, 1]).or_else(|| on([0, 0, 0, 0])))
}

/// IPv4 listening sockets: (address, port, owning process).
fn tcp_listeners() -> io::Result<Vec<([u8; 4], u16, u32)>> {
    unsafe {
        let mut size = 0u32;
        let _ = GetExtendedTcpTable(None, &mut size, false, AF_INET.0 as u32, TCP_TABLE_OWNER_PID_LISTENER, 0);
        // Sockets can open between the two calls: leave room, and retry.
        for _ in 0..4 {
            size += 4096;
            let mut buf = vec![0u32; size as usize / 4];
            let r = GetExtendedTcpTable(Some(buf.as_mut_ptr().cast()), &mut size, false, AF_INET.0 as u32, TCP_TABLE_OWNER_PID_LISTENER, 0);
            if r == 0 {
                return Ok(parse_table(bytemuck_u32(&buf)));
            }
            if r != windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER.0 {
                return Err(io::Error::from_raw_os_error(r as i32));
            }
        }
        Err(io::Error::other("the TCP table kept growing"))
    }
}

fn bytemuck_u32(buf: &[u32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), buf.len() * 4) }
}

/// A MIB_TCPTABLE_OWNER_PID: a u32 count, then rows of six u32s. Address
/// and port are in network byte order.
fn parse_table(bytes: &[u8]) -> Vec<([u8; 4], u16, u32)> {
    let row = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
    let Some(count) = bytes.get(..4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize) else {
        return Vec::new();
    };
    bytes[4..]
        .chunks_exact(row)
        .take(count)
        .map(|r| {
            let field = |i: usize| [r[i * 4], r[i * 4 + 1], r[i * 4 + 2], r[i * 4 + 3]];
            let addr = field(1);
            let port = u16::from_be_bytes([field(2)[0], field(2)[1]]);
            (addr, port, u32::from_ne_bytes(field(5)))
        })
        .collect()
}

/// The process of the running Apple Mobile Device Service.
fn service_pid() -> Option<u32> {
    unsafe {
        let scm = Handle(OpenSCManagerW(None, None, SC_MANAGER_CONNECT).ok()?);
        let svc = Handle(OpenServiceW(scm.0, w!("Apple Mobile Device Service"), SERVICE_QUERY_STATUS).ok()?);
        let mut status = SERVICE_STATUS_PROCESS::default();
        let mut needed = 0u32;
        let buf = std::slice::from_raw_parts_mut((&raw mut status).cast::<u8>(), std::mem::size_of::<SERVICE_STATUS_PROCESS>());
        QueryServiceStatusEx(svc.0, SC_STATUS_PROCESS_INFO, Some(buf), &mut needed).ok()?;
        (status.dwCurrentState == SERVICE_RUNNING && status.dwProcessId != 0).then_some(status.dwProcessId)
    }
}

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tcp_table_is_read_in_network_order() {
        let mut t = Vec::new();
        t.extend_from_slice(&2u32.to_ne_bytes());
        for (addr, port, pid) in [([127u8, 0, 0, 1], 27015u16, 5620u32), ([0, 0, 0, 0], 8765, 42)] {
            t.extend_from_slice(&2u32.to_ne_bytes()); // state: listen
            t.extend_from_slice(&addr);
            let p = port.to_be_bytes();
            t.extend_from_slice(&[p[0], p[1], 0, 0]);
            t.extend_from_slice(&[0; 8]); // remote address and port
            t.extend_from_slice(&pid.to_ne_bytes());
        }
        assert_eq!(parse_table(&t), vec![([127, 0, 0, 1], 27015, 5620), ([0, 0, 0, 0], 8765, 42)]);
    }

    /// On a PC with Apple's service running: `cargo test -p device -- --ignored`.
    #[test]
    #[ignore]
    fn apple_service_passes_on_this_pc() {
        check_listener().unwrap();
    }

    #[test]
    fn a_listener_is_traced_to_its_process() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert_eq!(listener_pid(port).unwrap(), Some(std::process::id()));
        drop(l);
        assert_eq!(listener_pid(port).unwrap(), None);
    }

    #[test]
    fn this_pc_answers_without_errors() {
        // Whatever this machine runs, the lookups themselves must work.
        assert!(tcp_listeners().is_ok());
        let _ = service_pid();
    }
}
