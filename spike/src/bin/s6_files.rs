//! s6: what can move files between the PC and the iPhone? Read-only survey:
//! which file/pasteboard services exist (lockdown and RSD), what the Photos
//! folder (AFC /DCIM) holds, which apps accept shared documents (house_arrest
//! VendDocuments), and the pasteboard's item *types* (never its contents).
//!
//!   s6_files [--pb-watch SECONDS]

use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use idevice::{
    IdeviceError, IdeviceService, ReadWrite, RsdService,
    xpc::RemoteXpcClient,
    afc::AfcClient,
    core_device::{DataInclusionPolicy, GENERAL_PASTEBOARD, PasteboardPayload, PasteboardServiceClient, PasteboardSnapshot},
    house_arrest::HouseArrestClient,
    installation_proxy::InstallationProxyClient,
};
use spike::conn::{NoDelayUsbmux, open_tunnel, usb_device};

#[derive(Parser)]
struct Args {
    /// After the survey, report pasteboard changes (types only) for this long.
    #[arg(long, default_value_t = 0.0)]
    pb_watch: f64,
    /// Put these PC files on the iPhone pasteboard (one item each, the UTI
    /// guessed from the extension), then show what the pasteboard holds.
    #[arg(long, num_args = 1..)]
    set: Vec<std::path::PathBuf>,
}

fn uti_for(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "public.png",
        Some("jpg" | "jpeg") => "public.jpeg",
        Some("heic") => "public.heic",
        Some("gif") => "com.compuserve.gif",
        Some("pdf") => "com.adobe.pdf",
        Some("txt") => "public.utf8-plain-text",
        Some("mp4") => "public.mpeg-4",
        Some("mov") => "com.apple.quicktime-movie",
        Some("zip") => "public.zip-archive",
        _ => "public.data",
    }
}

const RSD_WANTED: &[&str] = &[
    "com.apple.coredevice.pasteboardservice",
    "com.apple.coredevice.fileservice.control",
    "com.apple.coredevice.fileservice.data",
    "com.apple.afc.shim.remote",
    "com.apple.mobile.house_arrest.shim.remote",
    "com.apple.mobile.installation_proxy.shim.remote",
];

/// The pasteboard service on a bare XPC client, to see whole replies
/// (metadata included) rather than idevice's parsed view.
struct RawPb(RemoteXpcClient<Box<dyn ReadWrite>>);

impl RsdService for RawPb {
    fn rsd_service_name() -> std::borrow::Cow<'static, str> {
        "com.apple.coredevice.pasteboardservice".into()
    }
    async fn from_stream(stream: Box<dyn ReadWrite>) -> Result<Self, IdeviceError> {
        let mut c = RemoteXpcClient::new(stream).await?;
        c.do_handshake().await?;
        Ok(Self(c))
    }
}

impl RawPb {
    async fn pull_promised(&mut self) -> Result<plist::Value, IdeviceError> {
        let mut policy = plist::Dictionary::new();
        policy.insert("allPromised".into(), plist::Value::Dictionary(Default::default()));
        let mut msg = plist::Dictionary::new();
        msg.insert("command".into(), "PULL".into());
        msg.insert("pasteboardName".into(), GENERAL_PASTEBOARD.into());
        msg.insert("dataPolicy".into(), plist::Value::Dictionary(policy));
        self.0.send_object(plist::Value::Dictionary(msg), true).await?;
        self.0.recv().await
    }
}

/// Indented dump; data blobs become their length, long strings are cut.
fn dump(v: &plist::Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match v {
        plist::Value::Dictionary(d) => {
            for (k, v) in d {
                match v {
                    plist::Value::Dictionary(_) | plist::Value::Array(_) => {
                        *out += &format!("{pad}{k}:
");
                        dump(v, indent + 2, out);
                    }
                    _ => {
                        *out += &format!("{pad}{k}: ");
                        dump(v, 0, out);
                    }
                }
            }
        }
        plist::Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                *out += &format!("{pad}[{i}]
");
                dump(v, indent + 2, out);
            }
        }
        plist::Value::Data(d) => *out += &format!("{pad}<{} bytes>
", d.len()),
        plist::Value::String(s) if s.chars().count() > 60 => {
            *out += &format!("{pad}\"{}…\" ({} chars)
", s.chars().take(60).collect::<String>(), s.chars().count())
        }
        other => *out += &format!("{pad}{other:?}
"),
    }
}

fn describe(s: &PasteboardSnapshot) -> String {
    let mut out = format!("change {:?}, {} item(s)", s.change_count, s.items.len());
    for item in &s.items {
        out += &format!("\n    item {}:", item.index);
        for uti in &item.types {
            let size = match item.data.iter().find(|e| &e.uti == uti).map(|e| &e.payload) {
                Some(PasteboardPayload::Inline(b)) => format!("{} B inline", b.len()),
                Some(PasteboardPayload::Promised { size }) => format!("promised {size:?} B"),
                Some(PasteboardPayload::Error(e)) => format!("error {e}"),
                None => "-".into(),
            };
            out += &format!("\n      {uti}  [{size}]");
        }
    }
    out
}

#[tokio::main]
async fn main() -> Result<()> {
    spike::init_logging();
    let args = Args::parse();
    let dev = usb_device().await?;
    let provider = NoDelayUsbmux::new(&dev);

    // 1. Photos folder over lockdown AFC (/var/mobile/Media).
    println!("== AFC (lockdown com.apple.afc)");
    match AfcClient::connect(&provider).await {
        Ok(mut afc) => {
            println!("  / : {:?}", afc.list_dir("/").await?);
            let dcim = afc.list_dir("/DCIM").await.unwrap_or_default();
            let mut total = 0;
            let mut newest = None;
            for d in dcim.iter().filter(|d| !d.starts_with('.')) {
                let files = afc.list_dir(format!("/DCIM/{d}")).await.unwrap_or_default();
                let n = files.iter().filter(|f| !f.starts_with('.')).count();
                println!("  /DCIM/{d}: {n} files");
                total += n;
                newest = Some((d.clone(), files));
            }
            println!("  DCIM total {total} files");
            if let Some((d, mut files)) = newest {
                files.retain(|f| !f.starts_with('.'));
                files.sort();
                for f in files.iter().rev().take(4) {
                    let p = format!("/DCIM/{d}/{f}");
                    match afc.get_file_info(&p).await {
                        Ok(i) => println!("    {p}  {} B  modified {}", i.size, i.modified),
                        Err(e) => println!("    {p}  info error {e}"),
                    }
                }
            }
            for p in ["/Downloads", "/PhotoData", "/Purchases"] {
                match afc.list_dir(p).await {
                    Ok(l) => println!("  {p}: {} entries", l.len()),
                    Err(e) => println!("  {p}: {e}"),
                }
            }
        }
        Err(e) => println!("  connect failed: {e}"),
    }

    // 2. Apps that accept documents (what Finder's "Files" tab shows).
    println!("== apps with UIFileSharingEnabled (installation_proxy)");
    let mut sharing = Vec::new();
    match InstallationProxyClient::connect(&provider).await {
        Ok(mut ip) => {
            let apps = ip.get_apps(Some("Any"), None).await?;
            println!("  {} apps total", apps.len());
            for (id, info) in &apps {
                let d = info.as_dictionary();
                let flag = |k: &str| d.and_then(|d| d.get(k)).and_then(|v| v.as_boolean()).unwrap_or(false);
                if flag("UIFileSharingEnabled") {
                    let name = d.and_then(|d| d.get("CFBundleDisplayName").or(d.get("CFBundleName")))
                        .and_then(|v| v.as_string()).unwrap_or("?");
                    let kind = d.and_then(|d| d.get("ApplicationType")).and_then(|v| v.as_string()).unwrap_or("?");
                    println!("  {id} \"{name}\" ({kind}) in-place={}", flag("LSSupportsOpeningDocumentsInPlace"));
                    sharing.push(id.clone());
                }
            }
        }
        Err(e) => println!("  connect failed: {e}"),
    }
    sharing.sort();
    for id in sharing.iter().take(3).chain(["com.apple.DocumentsApp".to_string()].iter()) {
        let r = match HouseArrestClient::connect(&provider).await {
            Ok(h) => match h.vend_documents(id.clone()).await {
                Ok(mut afc) => afc.list_dir("/Documents").await.map(|l| format!("Documents: {} entries", l.len()))
                    .unwrap_or_else(|e| format!("list error {e}")),
                Err(e) => format!("VendDocuments refused: {e}"),
            },
            Err(e) => format!("house_arrest connect failed: {e}"),
        };
        println!("  house_arrest {id}: {r}");
    }

    // 3. RSD services and the pasteboard (types and sizes only).
    let mut t = open_tunnel(&provider).await?;
    println!("== RSD");
    for w in RSD_WANTED {
        println!("  [{}] {w}", if t.handshake.services.contains_key(*w) { "x" } else { " " });
    }
    let mut others: Vec<_> = t.handshake.services.keys()
        .filter(|k| k.contains("file") || k.contains("paste") || k.contains("afc") || k.contains("drag") || k.contains("drop"))
        .cloned().collect();
    others.sort();
    println!("  file/paste/afc/drag/drop services: {others:?}");

    println!("== pasteboard (allPromised: no contents are read)");
    let mut pb = PasteboardServiceClient::connect_rsd(&mut t.adapter, &mut t.handshake).await?;
    let a = Instant::now();
    let snap = pb.get_with_policy(GENERAL_PASTEBOARD, DataInclusionPolicy::AllPromised).await?;
    println!("  {} ({:.0} ms)", describe(&snap), a.elapsed().as_secs_f64() * 1e3);

    if !args.set.is_empty() {
        let mut items = Vec::new();
        for path in &args.set {
            let bytes = std::fs::read(path)?;
            let uti = uti_for(path);
            println!("  set {} as {uti} ({} B)", path.display(), bytes.len());
            let mut data = plist::Dictionary::new();
            let mut datum = plist::Dictionary::new();
            datum.insert("data".into(), plist::Value::Data(bytes));
            data.insert(uti.into(), plist::Value::Dictionary(datum));
            let mut item = plist::Dictionary::new();
            item.insert("types".into(), plist::Value::Array(vec![uti.into()]));
            item.insert("data".into(), plist::Value::Dictionary(data));
            items.push(plist::Value::Dictionary(item));
        }
        let mut msg = plist::Dictionary::new();
        msg.insert("command".into(), "SET".into());
        msg.insert("pasteboardName".into(), GENERAL_PASTEBOARD.into());
        msg.insert("items".into(), plist::Value::Array(items));
        let mut raw = RawPb::connect_rsd(&mut t.adapter, &mut t.handshake).await?;
        let a = Instant::now();
        let mut obj: idevice::xpc::XPCObject = plist::Value::Dictionary(msg).into();
        if let idevice::xpc::XPCObject::Dictionary(d) = &mut obj {
            d.insert("sourceMetadata".into(), idevice::xpc::XPCObject::Null);
        }
        raw.0.send_object(obj, true).await?;
        let mut out = String::new();
        dump(&raw.0.recv().await?, 4, &mut out);
        println!("  SET reply ({:.0} ms):
{out}", a.elapsed().as_secs_f64() * 1e3);
        let s = pb.get_with_policy(GENERAL_PASTEBOARD, DataInclusionPolicy::AllPromised).await?;
        println!("  now: {}", describe(&s));
    }

    if args.pb_watch > 0.0 {
        println!("== watching the pasteboard for {} s: copy something on the iPhone", args.pb_watch);
        let end = Instant::now() + Duration::from_secs_f64(args.pb_watch);
        let mut last = snap.change_count;
        while Instant::now() < end {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let s = pb.get_with_policy(GENERAL_PASTEBOARD, DataInclusionPolicy::AllPromised).await?;
            if s.change_count != last {
                last = s.change_count;
                println!("  {}", describe(&s));
                let mut raw = RawPb::connect_rsd(&mut t.adapter, &mut t.handshake).await?;
                let mut out = String::new();
                dump(&raw.pull_promised().await?, 4, &mut out);
                println!("  raw reply:
{out}");
                // Fetch each promised representation: size and leading bytes
                // only (text: up to 80 chars, expected to be a file name).
                for item in &s.items {
                    for uti in &item.types {
                        let a = Instant::now();
                        match pb.resolve(GENERAL_PASTEBOARD, item.index as i64, uti).await {
                            Ok(Some(b)) => {
                                let head: String = b.iter().take(8).map(|x| format!("{x:02x}")).collect();
                                let text = if uti.contains("text") {
                                    format!(" \"{}\"", String::from_utf8_lossy(&b).chars().take(80).collect::<String>())
                                } else { String::new() };
                                println!("    resolve item {} {uti}: {} B, head {head}{text} ({:.0} ms)",
                                    item.index, b.len(), a.elapsed().as_secs_f64() * 1e3);
                                if !uti.contains("text") {
                                    let f = std::env::temp_dir().join(format!("s6_item{}_{}.bin", item.index, uti.replace('.', "_")));
                                    let _ = std::fs::write(&f, &b);
                                }
                            }
                            Ok(None) => println!("    resolve item {} {uti}: no data", item.index),
                            Err(e) => println!("    resolve item {} {uti}: error {e}", item.index),
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
