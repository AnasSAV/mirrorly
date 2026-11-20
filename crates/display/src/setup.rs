//! One-time setup for the iPad display, done by an elevated copy of
//! Mirrorly (one administrator prompt, the first time "Use as display" is
//! chosen):
//!
//! - the Virtual Display Driver (MIT, signed by SignPath; bundled in a
//!   `vdd` folder next to Mirrorly.exe), with settings for one monitor at
//!   iPad resolutions, rendered on the Intel GPU when there is one: Quick
//!   Sync can then encode the monitor's picture where it is drawn (on a
//!   hybrid laptop the driver's "default" is the NVIDIA GPU, which Quick
//!   Sync can't read);
//! - an inbound firewall rule for Mirrorly's display server, so the iPad
//!   can reach it (private and domain networks, this program only).
//!
//! Driver installation follows what `devcon install MttVDD.inf
//! Root\MttVDD` does: the package goes into the driver store (pnputil),
//! a root-enumerated device is created, and the driver is installed on it.

use std::{
    fs::OpenOptions,
    io::Write,
    os::windows::{
        fs::OpenOptionsExt,
        io::{FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            DICD_GENERATE_ID, DICS_FLAG_CONFIGSPECIFIC, DICS_PROPCHANGE, DIF_PROPERTYCHANGE, DIF_REGISTERDEVICE, DIGCF_PRESENT,
            GUID_DEVCLASS_DISPLAY, INSTALLFLAG_FORCE, SP_CLASSINSTALL_HEADER, SP_DEVINFO_DATA, SP_PROPCHANGE_PARAMS,
            SPDRP_HARDWAREID, SetupDiCallClassInstaller, SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW,
            SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyW,
            SetupDiSetClassInstallParamsW, SetupDiSetDeviceRegistryPropertyW, UpdateDriverForPlugAndPlayDevicesW,
        },
        Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1},
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0},
        Security::{
            ACL, Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
                SetSecurityInfo,
            },
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, IsWellKnownSid, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, WinBuiltinAdministratorsSid,
            WinLocalSystemSid,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
            GetFileInformationByHandle, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
        },
        System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject},
        UI::{
            Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_HIDE,
        },
    },
    core::{BOOL, HRESULT, HSTRING, PCWSTR, w},
};

use crate::{PORT, screen::driver_present};

/// The command-line switch that runs `perform` (in the elevated copy).
pub const SETUP_ARG: &str = "--setup-display";

/// The driver's hardware ID (from its INF).
const HARDWARE_ID: &str = "Root\\MttVDD";
const FIREWALL_RULE: &str = "Mirrorly iPad display";
/// Where the driver reads its settings.
const SETTINGS_DIR: &str = r"C:\VirtualDisplayDriver";

/// iPad screens, landscape: 10th gen / Air 11", Air 13" / Pro 12.9",
/// Pro 11", Pro 11" (M4), Pro 13" (M4), mini (6th gen on), 9th gen.
const IPAD_SIZES: [(u32, u32); 7] = [(2360, 1640), (2732, 2048), (2388, 1668), (2420, 1668), (2752, 2064), (2266, 1488), (2160, 1620)];

/// Done for this copy of Mirrorly (the firewall rule names its path).
fn marker() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Mirrorly").join("display-setup.txt")
}

fn this_exe() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
}

/// Whether "Use as display" needs the setup first: no driver, a firewall
/// rule for another copy of Mirrorly, or the driver on the wrong GPU.
pub fn needed() -> bool {
    !driver_present()
        || std::fs::read_to_string(marker()).map(|s| s.trim() != this_exe()).unwrap_or(true)
        || configured_gpu().is_none_or(|g| g != preferred_gpu())
}

const INTEL: u32 = 0x8086;

/// The GPU the driver should render on: the Intel GPU when there is one
/// (Quick Sync encodes there without a copy), else the driver's default.
pub fn preferred_gpu() -> String {
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return "default".into() };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = adapter.GetDesc1() else { continue };
            if d.VendorId == INTEL && d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
                let name = &d.Description[..d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len())];
                return String::from_utf16_lossy(name);
            }
        }
    }
    "default".into()
}

/// The GPU named in the driver's settings now.
fn configured_gpu() -> Option<String> {
    let xml = std::fs::read_to_string(Path::new(SETTINGS_DIR).join("vdd_settings.xml")).ok()?;
    let start = xml.find("<friendlyname>")? + "<friendlyname>".len();
    let end = start + xml[start..].find("</friendlyname>")?;
    Some(xml[start..end].trim().to_string())
}

/// Run the setup in an elevated copy of this program, and wait for it
/// (the administrator prompt shows meanwhile). Ok(false): the user said no.
pub fn run_elevated(prefer: (u32, u32)) -> Result<bool> {
    let exe = HSTRING::from(this_exe());
    let params = HSTRING::from(format!("{SETUP_ARG} {}x{}", prefer.0, prefer.1));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(false);
            }
            return Err(e).context("start the setup as administrator");
        }
        if info.hProcess.is_invalid() {
            bail!("the setup did not start");
        }
        let waited = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let got = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 || got.is_err() || code != 0 {
            bail!("the setup failed (exit code {code}); details in logs\\display-setup.log");
        }
    }
    let m = marker();
    let _ = std::fs::create_dir_all(m.parent().unwrap_or(&m)).and_then(|_| std::fs::write(&m, this_exe()));
    // The new device takes a moment to show up.
    let start = Instant::now();
    while !driver_present() && start.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(true)
}

/// The elevated part. `size` is the iPad's resolution, "2360x1640".
pub fn perform(size: &str) -> Result<()> {
    let prefer = size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
    let gpu = preferred_gpu();
    let changed_gpu = configured_gpu().is_some_and(|g| g != gpu);
    write_settings(prefer, &gpu)?;
    if !driver_present() {
        install_driver(&driver_dir()?)?;
    } else if changed_gpu {
        // The driver reads its settings when it starts.
        restart_driver()?;
    } else {
        tracing::info!("the Virtual Display Driver is already installed");
    }
    firewall()?;
    tracing::info!("display setup done");
    Ok(())
}

/// The bundled driver: next to Mirrorly.exe, or in the source tree when
/// running a development build.
fn driver_dir() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("vdd"));
    }
    // Never in a release: the elevated setup must not look outside the
    // install folder (the build machine's path means nothing here, and
    // someone else could create it).
    #[cfg(debug_assertions)]
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/vdd"));
    candidates
        .into_iter()
        .find(|d| d.join("MttVDD.inf").is_file())
        .context("the Virtual Display Driver files (vdd folder) are missing next to Mirrorly.exe")
}

fn settings_xml(prefer: Option<(u32, u32)>, gpu: &str) -> String {
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for s in prefer.into_iter().chain(IPAD_SIZES).chain([(1920, 1080)]) {
        if !sizes.contains(&s) {
            sizes.push(s);
        }
    }
    let resolutions: String = sizes
        .iter()
        .map(|(w, h)| format!("        <resolution><width>{w}</width><height>{h}</height><refresh_rate>60</refresh_rate></resolution>\n"))
        .collect();
    format!(
        "<?xml version='1.0' encoding='utf-8'?>
<vdd_settings>
    <monitors>
        <count>1</count>
    </monitors>
    <gpu>
        <friendlyname>{gpu}</friendlyname>
    </gpu>
    <global>
        <g_refresh_rate>60</g_refresh_rate>
    </global>
    <resolutions>
{resolutions}    </resolutions>
    <options>
        <CustomEdid>false</CustomEdid>
        <PreventSpoof>false</PreventSpoof>
        <EdidCeaOverride>false</EdidCeaOverride>
        <HardwareCursor>true</HardwareCursor>
        <SDR10bit>false</SDR10bit>
        <HDRPlus>false</HDRPlus>
        <logging>false</logging>
        <debuglogging>false</debuglogging>
    </options>
</vdd_settings>
"
    )
}

/// One monitor, offering the iPad's resolution, on `gpu` (read by the
/// driver when it starts). Runs elevated, into a folder at the root of C:,
/// where any user may create folders: see `secure_dir`.
fn write_settings(prefer: Option<(u32, u32)>, gpu: &str) -> Result<()> {
    // Held open (no sharing for delete) until the file is written, so the
    // folder can't be renamed or swapped for a link meanwhile.
    let _dir = secure_dir(Path::new(SETTINGS_DIR))?;
    let file = Path::new(SETTINGS_DIR).join("vdd_settings.xml");
    match std::fs::remove_file(&file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("remove {}", file.display()));
        }
        _ => {}
    }
    // A new file, never through a link (the old name is gone: anything
    // there now was made in the window above, which the DACL closes).
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&file)
        .with_context(|| format!("create {}", file.display()))?;
    f.write_all(settings_xml(prefer, gpu).as_bytes()).with_context(|| format!("write {}", file.display()))?;
    tracing::info!("wrote {} (GPU: {gpu})", file.display());
    Ok(())
}

/// Administrators and SYSTEM change it; everyone else (the driver's host
/// runs as LOCAL SERVICE) may only read. Not inherited from C:\, where
/// every user may add and change folders.
const SETTINGS_DIR_SDDL: &str = "D:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1200a9;;;AU)(A;OICI;0x1200a9;;;LS)";

/// Make `dir` a folder that only administrators and SYSTEM can change, and
/// return a handle that keeps it in place. A new folder is created with
/// that DACL; an existing one is accepted only if it is a real folder (not
/// a junction or link to somewhere else) owned by Administrators or SYSTEM,
/// and gets that DACL. One made by another account is refused, as someone
/// may have prepared it to redirect this elevated write.
fn secure_dir(dir: &Path) -> Result<OwnedHandle> {
    let wide = HSTRING::from(dir.as_os_str());
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(SETTINGS_DIR_SDDL), SDDL_REVISION_1, &mut sd, None)
            .context("settings folder security descriptor")?;
        let result = (|| {
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            if let Err(e) = CreateDirectoryW(&wide, Some(&sa))
                && e.code() != ERROR_ALREADY_EXISTS.to_hresult()
            {
                return Err(e).with_context(|| format!("create {}", dir.display()));
            }
            let h = CreateFileW(
                &wide,
                (READ_CONTROL | WRITE_DAC).0 | FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                None,
            )
            .with_context(|| format!("open {}", dir.display()))?;
            let handle = OwnedHandle::from_raw_handle(h.0);
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            GetFileInformationByHandle(h, &mut info).context("settings folder attributes")?;
            if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
                bail!("{} is a link or not a folder: delete it, then try again", dir.display());
            }
            if !owned_by_admins(h)? {
                bail!("{} belongs to another account: delete it, then try again", dir.display());
            }
            let mut present = BOOL::default();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut defaulted = BOOL::default();
            GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted).context("settings folder DACL")?;
            SetSecurityInfo(
                h,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl),
                None,
            )
            .ok()
            .with_context(|| format!("set the permissions of {}", dir.display()))?;
            Ok(handle)
        })();
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        result
    }
}

/// Whether Administrators or SYSTEM own the object behind `h`.
fn owned_by_admins(h: HANDLE) -> Result<bool> {
    unsafe {
        let mut owner = PSID::default();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        GetSecurityInfo(h, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, Some(&mut owner), None, None, None, Some(&mut sd))
            .ok()
            .context("settings folder owner")?;
        let ok = IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool() || IsWellKnownSid(owner, WinLocalSystemSid).as_bool();
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        Ok(ok)
    }
}

/// Restart the driver's device(s), as `devcon restart Root\MttVDD` does.
fn restart_driver() -> Result<()> {
    unsafe {
        let set = SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_DISPLAY), None, None, DIGCF_PRESENT).context("list display devices")?;
        let mut restarted = 0;
        let mut i = 0;
        loop {
            let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, i, &mut data).is_err() {
                break;
            }
            i += 1;
            let mut buf = [0u8; 512];
            if SetupDiGetDeviceRegistryPropertyW(set, &data, SPDRP_HARDWAREID, None, Some(&mut buf), None).is_err() {
                continue;
            }
            let ids: Vec<u16> = buf.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            if !String::from_utf16_lossy(&ids).split('\0').any(|id| id.eq_ignore_ascii_case(HARDWARE_ID)) {
                continue;
            }
            let params = SP_PROPCHANGE_PARAMS {
                ClassInstallHeader: SP_CLASSINSTALL_HEADER {
                    cbSize: std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as u32,
                    InstallFunction: DIF_PROPERTYCHANGE,
                },
                StateChange: DICS_PROPCHANGE,
                Scope: DICS_FLAG_CONFIGSPECIFIC,
                HwProfile: 0,
            };
            let ok = SetupDiSetClassInstallParamsW(set, Some(&data), Some(&params.ClassInstallHeader), std::mem::size_of_val(&params) as u32)
                .and_then(|()| SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set, Some(&data)));
            match ok {
                Ok(()) => restarted += 1,
                Err(e) => tracing::warn!("restart the virtual display: {e}"),
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        if restarted == 0 {
            bail!("could not restart the Virtual Display Driver");
        }
        tracing::info!("restarted the Virtual Display Driver ({restarted} device)");
    }
    Ok(())
}

/// Run a system tool; its output goes to the log.
/// Run a Windows tool from System32 by its full path: by bare name, Windows
/// would look in Mirrorly's own folder first, which in a per-user install
/// any process of the user can write to, and this runs elevated.
fn run(program: &str, raw_args: &str) -> Result<i32> {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let path = root.join("System32").join(program);
    let out = Command::new(&path).raw_arg(raw_args).creation_flags(0x0800_0000).output().with_context(|| format!("run {program}"))?;
    let code = out.status.code().unwrap_or(-1);
    tracing::info!("{program} {raw_args} -> {code}\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok(code)
}

fn install_driver(dir: &Path) -> Result<()> {
    let inf = dir.join("MttVDD.inf").canonicalize().context("driver INF")?;
    // canonicalize gives a \\?\ path, which pnputil and newdev do not take.
    let inf = inf.display().to_string().trim_start_matches(r"\\?\").to_string();
    let code = run("pnputil.exe", &format!("/add-driver \"{inf}\""))?;
    if code != 0 {
        bail!("pnputil could not add the driver package (exit code {code})");
    }
    unsafe {
        let set = SetupDiCreateDeviceInfoList(Some(&GUID_DEVCLASS_DISPLAY), None).context("SetupDiCreateDeviceInfoList")?;
        let created = (|| -> Result<()> {
            let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            SetupDiCreateDeviceInfoW(set, w!("Display"), &GUID_DEVCLASS_DISPLAY, None, None, DICD_GENERATE_ID, Some(&mut data))
                .context("SetupDiCreateDeviceInfo")?;
            // REG_MULTI_SZ: the ID, then an empty string.
            let id: Vec<u8> = HARDWARE_ID.encode_utf16().chain([0, 0]).flat_map(u16::to_le_bytes).collect();
            SetupDiSetDeviceRegistryPropertyW(set, &mut data, SPDRP_HARDWAREID, Some(&id)).context("set the hardware ID")?;
            SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, Some(&data)).context("register the device")?;
            Ok(())
        })();
        let _ = SetupDiDestroyDeviceInfoList(set);
        created?;
        let mut reboot = BOOL(0);
        UpdateDriverForPlugAndPlayDevicesW(None, &HSTRING::from(HARDWARE_ID), &HSTRING::from(inf.as_str()), INSTALLFLAG_FORCE, Some(&mut reboot))
            .context("install the driver on the device")?;
        tracing::info!("Virtual Display Driver installed (restart needed: {})", reboot.as_bool());
    }
    Ok(())
}

/// Let the iPad reach the display server: this program, its port,
/// private and domain networks.
fn firewall() -> Result<()> {
    let exe = this_exe();
    let _ = run("netsh.exe", &format!("advfirewall firewall delete rule name=\"{FIREWALL_RULE}\""));
    let code = run(
        "netsh.exe",
        &format!(
            "advfirewall firewall add rule name=\"{FIREWALL_RULE}\" dir=in action=allow protocol=TCP localport={PORT} program=\"{exe}\" profile=private,domain enable=yes"
        ),
    )?;
    if code != 0 {
        bail!("netsh could not add the firewall rule (exit code {code})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not elevated, a folder is owned by the user who made it: exactly
    /// what the elevated setup must refuse, like a junction planted where
    /// the folder should be. Elevated (as on CI), the folder it makes is the
    /// administrators' and is kept.
    #[test]
    fn the_settings_folder_must_be_an_admin_owned_real_folder() {
        use std::os::windows::io::AsRawHandle;
        let base = std::env::temp_dir().join(format!("mirrorly-secure-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let made_here = base.join("made-here");
        match secure_dir(&made_here) {
            Ok(h) => assert!(owned_by_admins(HANDLE(h.as_raw_handle())).unwrap(), "elevated: owned by Administrators"),
            Err(e) => assert!(e.to_string().contains("belongs to another account"), "{e}"),
        }
        let target = base.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = base.join("junction");
        let made = Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
        assert!(made.status.success(), "mklink /J");
        let err = secure_dir(&link).unwrap_err().to_string();
        assert!(err.contains("is a link"), "{err}");
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn settings_list_the_ipad_first_once() {
        let xml = settings_xml(Some((2360, 1640)), "Intel(R) UHD Graphics");
        assert!(xml.contains("<friendlyname>Intel(R) UHD Graphics</friendlyname>"));
        assert_eq!(xml.matches("<width>2360</width><height>1640</height>").count(), 1);
        let first = xml.find("<resolution>").unwrap();
        assert!(xml[first..].starts_with("<resolution><width>2360</width>"));
        assert!(xml.contains("<count>1</count>"));
        let other = settings_xml(Some((1234, 800)), "default");
        assert!(other[other.find("<resolution>").unwrap()..].starts_with("<resolution><width>1234</width>"));
    }
}
