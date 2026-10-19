//! One-time setup for the iPad display, done by an elevated copy of
//! Mirrorly (one administrator prompt, the first time "Use as display" is
//! chosen):
//!
//! - the Virtual Display Driver (MIT, signed by SignPath; bundled in a
//!   `vdd` folder next to Mirrorly.exe), with settings for one monitor at
//!   iPad resolutions;
//! - an inbound firewall rule for Mirrorly's display server, so the iPad
//!   can reach it (private and domain networks, this program only).
//!
//! Driver installation follows what `devcon install MttVDD.inf
//! Root\MttVDD` does: the package goes into the driver store (pnputil),
//! a root-enumerated device is created, and the driver is installed on it.

use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            DICD_GENERATE_ID, DIF_REGISTERDEVICE, GUID_DEVCLASS_DISPLAY, INSTALLFLAG_FORCE, SP_DEVINFO_DATA,
            SPDRP_HARDWAREID, SetupDiCallClassInstaller, SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW,
            SetupDiDestroyDeviceInfoList, SetupDiSetDeviceRegistryPropertyW, UpdateDriverForPlugAndPlayDevicesW,
        },
        Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0},
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

/// Whether "Use as display" needs the setup first.
pub fn needed() -> bool {
    !driver_present() || std::fs::read_to_string(marker()).map(|s| s.trim() != this_exe()).unwrap_or(true)
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
    write_settings(prefer)?;
    if driver_present() {
        tracing::info!("the Virtual Display Driver is already installed");
    } else {
        install_driver(&driver_dir()?)?;
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
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/vdd"));
    candidates
        .into_iter()
        .find(|d| d.join("MttVDD.inf").is_file())
        .context("the Virtual Display Driver files (vdd folder) are missing next to Mirrorly.exe")
}

fn settings_xml(prefer: Option<(u32, u32)>) -> String {
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
        <friendlyname>default</friendlyname>
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

/// One monitor, offering the iPad's resolution (read by the driver when
/// it starts).
fn write_settings(prefer: Option<(u32, u32)>) -> Result<()> {
    std::fs::create_dir_all(SETTINGS_DIR).with_context(|| format!("create {SETTINGS_DIR}"))?;
    let file = Path::new(SETTINGS_DIR).join("vdd_settings.xml");
    std::fs::write(&file, settings_xml(prefer)).with_context(|| format!("write {}", file.display()))?;
    tracing::info!("wrote {}", file.display());
    Ok(())
}

/// Run a system tool; its output goes to the log.
fn run(program: &str, raw_args: &str) -> Result<i32> {
    let out = Command::new(program).raw_arg(raw_args).creation_flags(0x0800_0000).output().with_context(|| format!("run {program}"))?;
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

    #[test]
    fn settings_list_the_ipad_first_once() {
        let xml = settings_xml(Some((2360, 1640)));
        assert_eq!(xml.matches("<width>2360</width><height>1640</height>").count(), 1);
        let first = xml.find("<resolution>").unwrap();
        assert!(xml[first..].starts_with("<resolution><width>2360</width>"));
        assert!(xml.contains("<count>1</count>"));
        let other = settings_xml(Some((1234, 800)));
        assert!(other[other.find("<resolution>").unwrap()..].starts_with("<resolution><width>1234</width>"));
    }
}
