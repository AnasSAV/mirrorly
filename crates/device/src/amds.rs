//! Who answers on 127.0.0.1:27015. Mirrorly trusts that port as Apple
//! Mobile Device Service: the device list, pairing records and every stream
//! to a USB device go through it. Loopback is shared by every account on
//! the PC, so when Apple's service isn't holding the port, another user's
//! program could, and would become "the iPhone" (receiving the clipboard and
//! keystrokes). So each connection is checked: the process listening on the
//! port must be Apple's, in one of the two ways Apple ships it:
//!
//! - **Apple Mobile Device Support** (with iTunes, or on its own) installs
//!   the Windows service "Apple Mobile Device Service", which only an
//!   administrator can install.
//! - **Apple Devices** from the Microsoft Store runs the same thing
//!   (`AppleMobileDeviceProcess.exe`) from its package, with no Windows
//!   service. Its process is recognised by its package: Windows reports a
//!   packaged process's family name, and where the package is installed
//!   (under WindowsApps, which only the system writes to).

use std::io;

use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE},
        NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER},
        Networking::WinSock::AF_INET,
        Storage::Packaging::Appx::{GetPackageFamilyName, GetPackagePathByFullName, GetPackagesByPackageFamily},
        System::{
            Services::{
                CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE,
                SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
                SERVICE_STATUS_PROCESS,
            },
            Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
        },
    },
    core::{HSTRING, PCWSTR, PWSTR, w},
};

pub const PORT: u16 = 27015;

/// The package family of Apple Devices in the Microsoft Store (product
/// 9NP83LWLPZ9K, as Microsoft's Store catalog lists it). The part after
/// `_` is derived from the publisher's signing identity, so no package from
/// another publisher can have this family name.
const APPLE_DEVICES: &str = "AppleInc.AppleDevices_nzyj5cx40ttqa";

/// Fails unless Apple's service is what listens on the port: Apple Mobile
/// Device Service, or the Apple Devices app.
pub fn check_listener() -> io::Result<()> {
    let Some(pid) = listener_pid(PORT)? else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("nothing listens on 127.0.0.1:{PORT}: Apple Mobile Device Service is not running"),
        ));
    };
    if service_pid() == Some(pid) || from_package(pid, APPLE_DEVICES) {
        return Ok(());
    }
    let who = Process::open(pid)
        .and_then(|p| p.image())
        .map_or_else(|| format!("process {pid}"), |exe| format!("{exe} (process {pid})"));
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("127.0.0.1:{PORT} is held by {who}, which is neither Apple Mobile Device Service nor the Apple Devices app"),
    ))
}

/// Whether the process belongs to the package family: it runs with that
/// package's identity, or its program is inside where the package is
/// installed.
fn from_package(pid: u32, family: &str) -> bool {
    let Some(p) = Process::open(pid) else {
        return false;
    };
    if p.family().is_some_and(|f| f.eq_ignore_ascii_case(family)) {
        return true;
    }
    p.image().is_some_and(|exe| package_paths(family).iter().any(|dir| inside(&exe, dir)))
}

/// Whether `path` is in the folder `dir` (or below it).
fn inside(path: &str, dir: &str) -> bool {
    let (path, dir) = (path.to_lowercase(), dir.trim_end_matches('\\').to_lowercase());
    path.strip_prefix(&dir).is_some_and(|rest| rest.starts_with('\\'))
}

/// Where each installed package of the family is: Windows's own record.
fn package_paths(family: &str) -> Vec<String> {
    let family = HSTRING::from(family);
    let (mut count, mut len) = (0u32, 0u32);
    unsafe {
        if GetPackagesByPackageFamily(&family, &mut count, None, &mut len, None) != ERROR_INSUFFICIENT_BUFFER {
            return Vec::new(); // none installed
        }
        let mut names = vec![PWSTR::null(); count as usize];
        let mut buf = vec![0u16; len as usize];
        if GetPackagesByPackageFamily(&family, &mut count, Some(names.as_mut_ptr()), &mut len, Some(PWSTR(buf.as_mut_ptr())))
            != ERROR_SUCCESS
        {
            return Vec::new();
        }
        names
            .iter()
            .take(count as usize)
            .filter_map(|name| {
                let name = PCWSTR(name.0);
                let mut len = 0u32;
                if GetPackagePathByFullName(name, &mut len, None) != ERROR_INSUFFICIENT_BUFFER {
                    return None;
                }
                let mut path = vec![0u16; len as usize];
                if GetPackagePathByFullName(name, &mut len, Some(PWSTR(path.as_mut_ptr()))) != ERROR_SUCCESS {
                    return None;
                }
                Some(wide(&path))
            })
            .collect()
    }
}

/// A UTF-16 buffer up to its terminating NUL.
fn wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// A process opened only to read its program and package.
struct Process(HANDLE);

impl Process {
    fn open(pid: u32) -> Option<Self> {
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok().map(Process) }
    }

    /// The package family it runs as; None for a process without a package.
    fn family(&self) -> Option<String> {
        let mut len = 0u32;
        unsafe {
            if GetPackageFamilyName(self.0, &mut len, None) != ERROR_INSUFFICIENT_BUFFER {
                return None;
            }
            let mut buf = vec![0u16; len as usize];
            (GetPackageFamilyName(self.0, &mut len, Some(PWSTR(buf.as_mut_ptr()))) == ERROR_SUCCESS).then(|| wide(&buf))
        }
    }

    /// The full path of its program.
    fn image(&self) -> Option<String> {
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        unsafe { QueryFullProcessImageNameW(self.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()? };
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
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

    #[test]
    fn inside_means_in_the_folder() {
        let dir = r"C:\Program Files\WindowsApps\AppleInc.AppleDevices_1.1792.39681.0_x64__nzyj5cx40ttqa";
        assert!(inside(&format!(r"{dir}\AMDS\AppleMobileDeviceProcess.exe"), dir));
        assert!(inside(&format!(r"{}\x.exe", dir.to_uppercase()), dir), "paths compare without case");
        assert!(inside(&format!(r"{dir}\x.exe"), &format!(r"{dir}\")), "a trailing backslash on the folder");
        assert!(!inside(&format!(r"{dir}x\x.exe"), dir), "a folder whose name only starts the same");
        assert!(!inside(dir, dir), "the folder itself is no program in it");
        assert!(!inside(r"C:\Users\me\AppleMobileDeviceProcess.exe", dir));
    }

    #[test]
    fn a_process_without_a_package_is_not_apple_devices() {
        assert!(!from_package(std::process::id(), APPLE_DEVICES));
        assert_eq!(Process::open(std::process::id()).and_then(|p| p.family()), None);
    }

    /// The package checks against a packaged process every Windows 11 PC
    /// runs: `cargo test -p device -- --ignored`.
    #[test]
    #[ignore]
    fn a_packaged_process_is_recognised() {
        const START: &str = "Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy";
        let out = std::process::Command::new("tasklist")
            .args(["/fi", "imagename eq StartMenuExperienceHost.exe", "/fo", "csv", "/nh"])
            .output()
            .unwrap();
        let line = String::from_utf8_lossy(&out.stdout);
        let pid: u32 = line.split(',').nth(1).unwrap().trim_matches('"').parse().unwrap();
        let p = Process::open(pid).unwrap();
        assert_eq!(p.family().as_deref(), Some(START));
        assert!(from_package(pid, START));
        let exe = p.image().unwrap();
        assert!(package_paths(START).iter().any(|dir| inside(&exe, dir)), "{exe} is in the package's folder");
        assert!(!from_package(pid, APPLE_DEVICES));
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
