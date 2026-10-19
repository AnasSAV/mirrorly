//! File transfer through the iPhone's clipboard (CoreDevice
//! `com.apple.coredevice.pasteboardservice`, the service behind `devicectl
//! device pasteboard`).
//!
//! iPhone -> PC: when the phone's clipboard gets photos or files (Copy in
//! Photos or Files), they are fetched into `%LOCALAPPDATA%\Mirrorly\clipboard`
//! and offered to the window, which lets them be dragged out (and puts them
//! on the Windows clipboard). Copied text is read from the copy's primary
//! type only (the `promiseSecondary` policy), which the daemon answers at
//! once; see the stall note below.
//! PC -> iPhone, clipboard: text and images copied on Windows are put on the
//! phone's clipboard as they are copied ([`ClipCmd::Push`]), so a paste on
//! the iPhone (Ctrl+V in Mirrorly, or touch and hold › Paste) gets them.
//!
//! PC -> iPhone: files dropped on the window go onto the phone's clipboard.
//! Photos and videos are then pasted (Command-V) into whatever is open there;
//! documents are also saved by name where Files shows them (see `docs`),
//! since a paste loses the name.
//!
//! Wire format (verified 2026-10-18 on iOS 27 with spike s6_files; see also
//! pymobiledevice3's pasteboard_service.py): plain XPC dicts keyed by
//! `command`. PULL with the `allPromised` policy lists item types without
//! loading any data; RESOLVE fetches one (item, type). A photo copied in
//! Photos is one `public.jpeg` item per photo (HEIC is converted by iOS). A
//! file copied in Files is an item whose first type is a record of the file,
//! followed by the type holding its contents: `com.apple.DocumentManager.
//! FPItem.File` (an NSKeyedArchiver FPItem, real name in `_filename`; seen
//! with a .txt and `public.plain-text`) or `...FINode.File` (an archive
//! holding the file's `file://` URL; seen with PDFs and `com.adobe.pdf`). SET with one inline item per file is
//! accepted, and pastes in Notes and Files (Files names it by type, e.g.
//! "PNG image"; the pasteboard carries no file name).
//!
//! Only items with photo, video or file types are fetched: resolving other
//! copies can stall the daemon for a minute (Notes' `public.rtf`, per
//! pymobiledevice3), so text-only copies are never touched.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use idevice::{
    IdeviceError, ReadWrite, RsdService,
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
    xpc::{Dictionary, RemoteXpcClient, XPCObject},
};
use input::HidEvent;
use tokio::sync::mpsc;

use crate::{docs, hid::Stamped};

const SERVICE: &str = "com.apple.coredevice.pasteboardservice";
const GENERAL: &str = "general";
/// Files' record of a copied file: `com.apple.DocumentManager.FPItem.File`
/// or `com.apple.DocumentManager.FINode.File`.
fn is_record(uti: &str) -> bool {
    uti.starts_with("com.apple.DocumentManager.") && uti.ends_with(".File")
}
/// How often the phone's clipboard is checked (a PULL without data is a few
/// ms on the phone).
const POLL: Duration = Duration::from_millis(800);
/// Total size of one drop sent to the phone (it travels as one message).
pub const MAX_PASTE_BYTES: u64 = 200 * 1024 * 1024;

/// Photo and video types iOS puts on the clipboard, best first, with the
/// file extension to save them under.
const MEDIA: &[(&str, &str)] = &[
    ("public.heic", "heic"),
    ("public.jpeg", "jpg"),
    ("public.png", "png"),
    ("com.compuserve.gif", "gif"),
    ("org.webmproject.webp", "webp"),
    ("public.tiff", "tiff"),
    ("com.apple.quicktime-movie", "mov"),
    ("public.mpeg-4", "mp4"),
];

/// Clipboard activity reported to the window.
#[derive(Debug, Clone)]
pub enum ClipEvent {
    /// Something was copied on the iPhone and is being fetched.
    Fetching(usize),
    /// Copied on the iPhone, now on this PC: ready to drag out.
    Copied(Vec<PathBuf>),
    /// PC files arrived: `pasted` photos/videos went in with Command-V,
    /// `saved` documents were stored by name at `place` (and are on the
    /// clipboard too).
    Sent { pasted: usize, saved: usize, place: Option<String> },
    /// Something new is on the iPhone's clipboard (anything, text too): it
    /// is now newer than the PC's.
    PhoneCopied,
    /// Text copied on the iPhone, for the Windows clipboard.
    PhoneText(String),
    /// Windows clipboard content is on the iPhone's clipboard; `pasted`
    /// when Command-V followed.
    Pushed { pasted: bool },
    Failed(String),
}

/// Windows clipboard content for the iPhone's clipboard.
#[derive(Debug, Clone)]
pub enum PushContent {
    Text(String),
    /// PNG-encoded image.
    Png(Vec<u8>),
}

pub enum ClipCmd {
    /// Put these files on the phone's clipboard and press Command-V.
    Paste(Vec<PathBuf>),
    /// Put this on the phone's clipboard; then press V if `paste` (the
    /// user is holding Ctrl, which the phone already has as Command).
    Push(PushContent, bool),
}

/// The pasteboard types a pushed item is offered under (as iOS's own
/// copies of text do).
fn push_types(c: PushContent) -> Vec<(&'static str, Vec<u8>)> {
    match c {
        PushContent::Text(t) => {
            let b = t.into_bytes();
            vec![("public.utf8-plain-text", b.clone()), ("public.plain-text", b.clone()), ("public.text", b)]
        }
        PushContent::Png(b) => vec![("public.png", b)],
    }
}

/// The pasteboard service on a bare XPC client (idevice's client sets one
/// item at most and drops the reply metadata).
struct Pasteboard(RemoteXpcClient<Box<dyn ReadWrite>>);

impl RsdService for Pasteboard {
    fn rsd_service_name() -> std::borrow::Cow<'static, str> {
        SERVICE.into()
    }
    async fn from_stream(stream: Box<dyn ReadWrite>) -> Result<Self, IdeviceError> {
        let mut c = RemoteXpcClient::new(stream).await?;
        c.do_handshake().await?;
        Ok(Self(c))
    }
}

/// One clipboard item: its types (best first) and the change it belongs to.
#[derive(Debug, Clone)]
struct Item {
    index: i64,
    types: Vec<String>,
}

#[derive(Debug, Clone)]
struct Snapshot {
    change: Option<i64>,
    items: Vec<Item>,
}

fn dict(pairs: Vec<(&str, XPCObject)>) -> XPCObject {
    let mut d = Dictionary::new();
    for (k, v) in pairs {
        d.insert(k.to_string(), v);
    }
    XPCObject::Dictionary(d)
}

fn change_count(reply: &plist::Value) -> Option<i64> {
    let p = reply.as_dictionary()?.get("pasteboard")?.as_dictionary()?;
    p.get("metadata")?.as_dictionary()?.get("changeCount")?.as_signed_integer()
}

impl Pasteboard {
    async fn request(&mut self, msg: XPCObject) -> Result<plist::Value> {
        self.0.send_object(msg, true).await?;
        Ok(self.0.recv().await?)
    }

    /// Text of the copy, from each item's primary type only (inline with the
    /// `promiseSecondary` policy, answered at once even for a Notes copy).
    async fn primary_text(&mut self) -> Result<Option<String>> {
        let reply = self
            .request(dict(vec![
                ("command", XPCObject::String("PULL".into())),
                ("pasteboardName", XPCObject::String(GENERAL.into())),
                ("dataPolicy", dict(vec![("promiseSecondary", dict(vec![]))])),
            ]))
            .await?;
        Ok(inline_text(&reply))
    }

    /// Item types only; no data is loaded on the phone.
    async fn list(&mut self) -> Result<Snapshot> {
        let reply = self
            .request(dict(vec![
                ("command", XPCObject::String("PULL".into())),
                ("pasteboardName", XPCObject::String(GENERAL.into())),
                ("dataPolicy", dict(vec![("allPromised", dict(vec![]))])),
            ]))
            .await?;
        let items = reply
            .as_dictionary()
            .and_then(|d| d.get("pasteboard"))
            .and_then(|p| p.as_dictionary())
            .and_then(|p| p.get("items"))
            .and_then(|i| i.as_array())
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, item)| Item {
                        index: i as i64,
                        types: item
                            .as_dictionary()
                            .and_then(|d| d.get("types"))
                            .and_then(|t| t.as_array())
                            .map(|t| t.iter().filter_map(|v| v.as_string().map(str::to_string)).collect())
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Snapshot { change: change_count(&reply), items })
    }

    async fn resolve(&mut self, index: i64, uti: &str) -> Result<Vec<u8>> {
        let reply = self
            .request(dict(vec![
                ("command", XPCObject::String("RESOLVE".into())),
                ("pasteboardName", XPCObject::String(GENERAL.into())),
                ("itemIndex", XPCObject::Int64(index)),
                ("type", XPCObject::String(uti.into())),
            ]))
            .await?;
        reply
            .as_dictionary()
            .and_then(|d| d.get("data"))
            .and_then(|d| d.as_data())
            .map(|d| d.to_vec())
            .ok_or_else(|| anyhow!("the iPhone no longer has {uti} (copied something else?)"))
    }

    /// Replace the clipboard with these items, each offered under one or
    /// more (type, bytes); returns the new change count.
    async fn set(&mut self, items: Vec<Vec<(&'static str, Vec<u8>)>>) -> Result<Option<i64>> {
        let items = items
            .into_iter()
            .map(|reps| {
                let mut data = Dictionary::new();
                let types = reps.iter().map(|(uti, _)| XPCObject::String((*uti).into())).collect();
                for (uti, bytes) in reps {
                    data.insert(uti.into(), dict(vec![("data", XPCObject::Data(bytes))]));
                }
                dict(vec![("types", XPCObject::Array(types)), ("data", XPCObject::Dictionary(data))])
            })
            .collect();
        let reply = self
            .request(dict(vec![
                ("command", XPCObject::String("SET".into())),
                ("pasteboardName", XPCObject::String(GENERAL.into())),
                ("items", XPCObject::Array(items)),
                ("sourceMetadata", XPCObject::Null),
            ]))
            .await?;
        Ok(change_count(&reply))
    }
}

/// The first text type carried inline by a PULL reply.
fn inline_text(reply: &plist::Value) -> Option<String> {
    let items = reply.as_dictionary()?.get("pasteboard")?.as_dictionary()?.get("items")?.as_array()?;
    items.iter().find_map(|item| {
        let data = item.as_dictionary()?.get("data")?.as_dictionary()?;
        ["public.utf8-plain-text", "public.plain-text", "public.text"].iter().find_map(|uti| {
            let bytes = data.get(*uti)?.as_dictionary()?.get("data")?.as_data()?;
            String::from_utf8(bytes.to_vec()).ok().filter(|t| !t.is_empty())
        })
    })
}

/// Largest text handed to the Windows clipboard.
const MAX_TEXT: usize = 8 << 20;

/// What a clipboard item would become as a file on the PC.
#[derive(Debug, PartialEq)]
enum Plan<'a> {
    /// A file from the Files app: its record type, then its contents type.
    File { record: &'a str, contents: &'a str },
    Media { uti: &'a str, ext: &'static str },
}

fn plan(item: &Item) -> Option<Plan<'_>> {
    if let Some(record) = item.types.iter().find(|t| is_record(t)) {
        let contents = item.types.iter().find(|t| !is_record(t))?;
        return Some(Plan::File { record, contents });
    }
    // The item's own order says which representation is the original.
    item.types.iter().find_map(|t| MEDIA.iter().find(|(u, _)| u == t).map(|&(uti, ext)| Plan::Media { uti, ext }))
}

/// The file name in Files' record of a copied file: an FPItem's
/// `_filename`, else the last part of the `file://` URL an FINode holds.
fn record_filename(archive: &[u8]) -> Option<String> {
    let v = plist::Value::from_reader(std::io::Cursor::new(archive)).ok()?;
    let objects = v.as_dictionary()?.get("$objects")?.as_array()?;
    let deref = |v: &plist::Value| -> Option<String> {
        let v = match v {
            plist::Value::Uid(u) => objects.get(u.get() as usize)?,
            other => other,
        };
        match v {
            plist::Value::String(s) => Some(s.clone()),
            plist::Value::Dictionary(d) => d.get("NS.string")?.as_string().map(str::to_string),
            _ => None,
        }
    };
    let named = objects.iter().filter_map(|o| o.as_dictionary()).find_map(|d| d.get("_filename").and_then(deref));
    named.or_else(|| {
        objects.iter().filter_map(|o| o.as_string()).find(|s| s.starts_with("file://")).and_then(|url| {
            let last = url.trim_end_matches('/').rsplit('/').next()?;
            Some(percent_decode(last)).filter(|n| !n.is_empty())
        })
    })
}

/// `%20` and friends back to UTF-8 text.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Photos, videos and audio paste well anywhere; the rest are documents.
fn is_media(uti: &str) -> bool {
    matches!(
        uti,
        "public.jpeg"
            | "public.png"
            | "public.heic"
            | "public.heif"
            | "com.compuserve.gif"
            | "org.webmproject.webp"
            | "public.tiff"
            | "com.microsoft.bmp"
            | "com.apple.quicktime-movie"
            | "public.mpeg-4"
    )
}

/// A name that is safe on Windows: no path parts, no reserved characters.
fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "iPhone file".into() } else { s }
}

/// `dir/name`, or `dir/stem (2).ext` and so on if that is taken.
fn unique(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap()
}

/// The clipboard's type for a PC file, from its extension. These match what
/// iOS itself uses; anything unknown goes as generic data.
pub fn uti_for(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("jpg" | "jpeg" | "jfif") => "public.jpeg",
        Some("png") => "public.png",
        Some("heic") => "public.heic",
        Some("heif") => "public.heif",
        Some("gif") => "com.compuserve.gif",
        Some("webp") => "org.webmproject.webp",
        Some("tif" | "tiff") => "public.tiff",
        Some("bmp") => "com.microsoft.bmp",
        Some("mov") => "com.apple.quicktime-movie",
        Some("mp4" | "m4v") => "public.mpeg-4",
        Some("mp3") => "public.mp3",
        Some("m4a") => "com.apple.m4a-audio",
        Some("wav") => "com.microsoft.waveform-audio",
        Some("pdf") => "com.adobe.pdf",
        Some("txt" | "log" | "md") => "public.utf8-plain-text",
        Some("csv") => "public.comma-separated-values-text",
        Some("json") => "public.json",
        Some("html" | "htm") => "public.html",
        Some("rtf") => "public.rtf",
        Some("zip") => "public.zip-archive",
        Some("docx") => "org.openxmlformats.wordprocessingml.document",
        Some("xlsx") => "org.openxmlformats.spreadsheetml.sheet",
        Some("pptx") => "org.openxmlformats.presentationml.presentation",
        Some("doc") => "com.microsoft.word.doc",
        Some("xls") => "com.microsoft.excel.xls",
        Some("ppt") => "com.microsoft.powerpoint.ppt",
        Some("pages") => "com.apple.iwork.pages.sffpages",
        Some("numbers") => "com.apple.iwork.numbers.sffnumbers",
        Some("key") => "com.apple.iwork.keynote.sffkey",
        _ => "public.data",
    }
}

/// Where fetched clipboard files go. Only the latest copy is kept.
pub fn inbox() -> PathBuf {
    crate::paths::data_dir().join("clipboard")
}

pub struct Bridge {
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    hid: mpsc::UnboundedSender<Stamped>,
    now_us: fn() -> u64,
    on_event: Box<dyn Fn(ClipEvent) + Send + Sync>,
    client: Option<Pasteboard>,
    /// Where documents are saved by name (looked up once per session).
    docs_host: Option<Option<docs::Host>>,
    /// Last change count seen; the first look only sets it, so whatever was
    /// already copied before connecting is not offered.
    seen: Option<Option<i64>>,
}

impl Bridge {
    pub fn new(
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        hid: mpsc::UnboundedSender<Stamped>,
        now_us: fn() -> u64,
        on_event: impl Fn(ClipEvent) + Send + Sync + 'static,
    ) -> Self {
        Self { adapter, handshake, hid, now_us, on_event: Box::new(on_event), client: None, docs_host: None, seen: None }
    }

    async fn client(&mut self) -> Result<&mut Pasteboard> {
        if self.client.is_none() {
            let c = Pasteboard::connect_rsd(&mut self.adapter, &mut self.handshake).await.context("pasteboard service")?;
            self.client = Some(c);
        }
        Ok(self.client.as_mut().unwrap())
    }

    /// Run `f` on the client with a deadline. Any failure drops the
    /// connection (a late reply would otherwise answer the next request).
    async fn with<T>(
        &mut self,
        limit: Duration,
        f: impl AsyncFnOnce(&mut Pasteboard) -> Result<T>,
    ) -> Result<T> {
        let c = self.client().await?;
        let r = match tokio::time::timeout(limit, f(c)).await {
            Ok(r) => r,
            Err(_) => Err(anyhow!("the iPhone's clipboard did not answer in {} s", limit.as_secs())),
        };
        if r.is_err() {
            self.client = None;
        }
        r
    }

    pub async fn run(mut self, cmds: &mut mpsc::UnboundedReceiver<ClipCmd>) {
        let mut tick = tokio::time::interval(POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                Some(cmd) = cmds.recv() => match cmd {
                    ClipCmd::Paste(files) => {
                        if let Err(e) = self.paste(files).await {
                            tracing::warn!("paste to iPhone failed: {e:#}");
                            (self.on_event)(ClipEvent::Failed(format!("{e:#}")));
                        }
                    }
                    ClipCmd::Push(content, paste) => {
                        if let Err(e) = self.push(content, paste).await {
                            tracing::warn!("clipboard to iPhone failed: {e:#}");
                            (self.on_event)(ClipEvent::Failed(format!("{e:#}")));
                        }
                    }
                },
                _ = tick.tick() => {
                    if let Err(e) = self.check().await {
                        tracing::warn!("iPhone clipboard: {e:#}");
                        (self.on_event)(ClipEvent::Failed(format!("{e:#}")));
                    }
                }
            }
        }
    }

    /// Look for a new copy on the phone and fetch it if it has files.
    async fn check(&mut self) -> Result<()> {
        let snap = self.with(Duration::from_secs(5), async |c| c.list().await).await?;
        if self.seen.is_none() {
            self.seen = Some(snap.change);
            return Ok(());
        }
        if self.seen == Some(snap.change) {
            return Ok(());
        }
        self.seen = Some(snap.change);
        (self.on_event)(ClipEvent::PhoneCopied);
        let plans: Vec<(i64, Plan)> = snap.items.iter().filter_map(|i| plan(i).map(|p| (i.index, p))).collect();
        if plans.is_empty() {
            // Text (or something else): only what the primary type carries.
            if snap.items.iter().any(|i| i.types.iter().any(|t| t.contains("text"))) {
                let text = self.with(Duration::from_secs(5), async |c| c.primary_text().await).await?;
                if let Some(t) = text.filter(|t| t.len() <= MAX_TEXT) {
                    tracing::info!("iPhone copied text ({} chars)", t.chars().count());
                    (self.on_event)(ClipEvent::PhoneText(t));
                }
            }
            return Ok(());
        }
        tracing::info!("iPhone clipboard change {:?}: {} file(s)", snap.change, plans.len());
        (self.on_event)(ClipEvent::Fetching(plans.len()));

        let dir = inbox().join(snap.change.unwrap_or(0).to_string());
        let _ = std::fs::remove_dir_all(inbox());
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let stamp = chrono::Local::now().format("%Y-%m-%d at %H.%M.%S").to_string();
        let mut files = Vec::new();
        for (n, (index, plan)) in plans.iter().enumerate() {
            let (name, bytes) = match plan {
                Plan::File { record, contents } => {
                    let record = self.with(Duration::from_secs(20), async |c| c.resolve(*index, record).await).await?;
                    let name = record_filename(&record).map(|n| safe_name(&n)).unwrap_or_else(|| "iPhone file".into());
                    let bytes = self.with(Duration::from_secs(120), async |c| c.resolve(*index, contents).await).await?;
                    (name, bytes)
                }
                Plan::Media { uti, ext } => {
                    let bytes = self.with(Duration::from_secs(120), async |c| c.resolve(*index, uti).await).await?;
                    let kind = if matches!(*ext, "mov" | "mp4") { "Video" } else { "Photo" };
                    let name = if plans.len() > 1 { format!("{kind} {stamp} ({}).{ext}", n + 1) } else { format!("{kind} {stamp}.{ext}") };
                    (name, bytes)
                }
            };
            let path = unique(&dir, &name);
            std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
            tracing::info!("fetched {} ({} B)", path.display(), bytes.len());
            files.push(path);
        }
        (self.on_event)(ClipEvent::Copied(files));
        Ok(())
    }

    /// PC files -> the phone's clipboard. Photos and videos are pasted into
    /// the open app with Command-V; documents are saved by name where Files
    /// shows them (and left on the clipboard for pasting by hand).
    async fn paste(&mut self, files: Vec<PathBuf>) -> Result<()> {
        let mut total = 0u64;
        for f in &files {
            let meta = std::fs::metadata(f).with_context(|| format!("{}", f.display()))?;
            if meta.is_dir() {
                bail!("folders can't be sent; drop the files inside it");
            }
            total += meta.len();
        }
        if total > MAX_PASTE_BYTES {
            bail!("too large to send at once ({} MB; at most {} MB)", total >> 20, MAX_PASTE_BYTES >> 20);
        }
        let mut loaded = Vec::new();
        for f in &files {
            loaded.push((f.as_path(), std::fs::read(f).with_context(|| format!("read {}", f.display()))?));
        }
        let docs: Vec<(&Path, Vec<u8>)> =
            loaded.iter().filter(|(f, _)| !is_media(uti_for(f))).map(|(f, b)| (*f, b.clone())).collect();
        let mut place = None;
        if !docs.is_empty() {
            if self.docs_host.is_none() {
                let host = docs::find_host(&mut self.adapter, &mut self.handshake).await;
                self.docs_host = Some(host.unwrap_or_else(|e| {
                    tracing::warn!("no app to keep documents in: {e:#}");
                    None
                }));
            }
            match self.docs_host.clone().flatten() {
                Some(host) => {
                    docs::save(&mut self.adapter, &mut self.handshake, &host, &docs).await?;
                    place = Some(host.place());
                }
                None => tracing::info!("no app shares a Files folder; documents go on the clipboard only"),
            }
        }
        let saved = if place.is_some() { docs.len() } else { 0 };
        let media = loaded.len() - docs.len();
        let items: Vec<_> = loaded.into_iter().map(|(f, b)| vec![(uti_for(f), b)]).collect();
        let change = self.with(Duration::from_secs(120), async |c| c.set(items).await).await?;
        // Our own copy is not news.
        self.seen = Some(change);
        tracing::info!("{} file(s) ({total} B) on the iPhone clipboard, change {change:?}", files.len());
        // Command-V (Left GUI + V), as from a hardware keyboard, when it is
        // all photos and videos (a document is already saved by name, and
        // pasting it into Files would make a nameless second copy).
        let pasted = if docs.is_empty() {
            let t = (self.now_us)();
            for (usage, down) in [(0xE3, true), (0x19, true), (0x19, false), (0xE3, false)] {
                let _ = self.hid.send((t, HidEvent::Key { usage, down }));
            }
            media
        } else {
            0
        };
        (self.on_event)(ClipEvent::Sent { pasted, saved, place });
        Ok(())
    }

    /// Windows clipboard -> the phone's clipboard, then V if asked (Command
    /// is the user's Ctrl, already held on the phone).
    async fn push(&mut self, content: PushContent, paste: bool) -> Result<()> {
        let kind = match &content {
            PushContent::Text(t) => format!("text ({} chars)", t.chars().count()),
            PushContent::Png(b) => format!("image ({} B)", b.len()),
        };
        let items = vec![push_types(content)];
        let change = self.with(Duration::from_secs(30), async |c| c.set(items).await).await?;
        self.seen = Some(change);
        tracing::info!("Windows clipboard {kind} on the iPhone, change {change:?}");
        if paste {
            let t = (self.now_us)();
            for (usage, down) in [(0xE3, true), (0x19, true), (0x19, false)] {
                let _ = self.hid.send((t, HidEvent::Key { usage, down }));
            }
        }
        (self.on_event)(ClipEvent::Pushed { pasted: paste });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(types: &[&str]) -> Item {
        Item { index: 0, types: types.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn plans_follow_what_ios_puts_on_the_clipboard() {
        // Seen on iOS 27: Photos "Copy Photo", Files "Copy" of a .txt.
        assert_eq!(plan(&item(&["public.jpeg"])), Some(Plan::Media { uti: "public.jpeg", ext: "jpg" }));
        const FP: &str = "com.apple.DocumentManager.FPItem.File";
        const FI: &str = "com.apple.DocumentManager.FINode.File";
        assert_eq!(plan(&item(&[FP, "public.plain-text"])), Some(Plan::File { record: FP, contents: "public.plain-text" }));
        // Files "Copy" / "Share > Copy" of a PDF.
        assert_eq!(plan(&item(&[FI, "com.adobe.pdf"])), Some(Plan::File { record: FI, contents: "com.adobe.pdf" }));
        // Text copies are never fetched.
        assert_eq!(plan(&item(&["public.utf8-plain-text", "public.text"])), None);
        assert_eq!(plan(&item(&[FP])), None);
        // The item's own order wins.
        assert_eq!(plan(&item(&["public.png", "public.jpeg"])), Some(Plan::Media { uti: "public.png", ext: "png" }));
    }

    #[test]
    fn names_are_windows_safe_and_unique() {
        assert_eq!(safe_name("fibonacci heap.txt"), "fibonacci heap.txt");
        assert_eq!(safe_name("a/b:c?.txt"), "b_c_.txt");
        assert_eq!(safe_name("  ..  "), "iPhone file");
        let dir = std::env::temp_dir().join(format!("mirrorly-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.txt"), b"").unwrap();
        assert_eq!(unique(&dir, "x.txt"), dir.join("x (2).txt"));
        assert_eq!(unique(&dir, "y.txt"), dir.join("y.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finode_filename_from_url() {
        let mut node = plist::Dictionary::new();
        node.insert("FI URL".into(), plist::Value::Uid(plist::Uid::new(2)));
        let mut d = plist::Dictionary::new();
        d.insert(
            "$objects".into(),
            plist::Value::Array(vec![
                "$null".into(),
                plist::Value::Dictionary(node),
                "file:///private/var/mobile/Containers/Shared/AppGroup/X/File%20Provider%20Storage/My%20report%E2%80%93v2.pdf".into(),
            ]),
        );
        let mut buf = Vec::new();
        plist::Value::Dictionary(d).to_writer_binary(&mut buf).unwrap();
        assert_eq!(record_filename(&buf).as_deref(), Some("My report–v2.pdf"));
        assert_eq!(percent_decode("a%2"), "a%2");
    }

    #[test]
    fn fp_item_filename() {
        // Shape of the archive Files puts on the clipboard (trimmed).
        let mut top = plist::Dictionary::new();
        top.insert("root".into(), plist::Value::Uid(plist::Uid::new(1)));
        let mut item = plist::Dictionary::new();
        item.insert("_filename".into(), plist::Value::Uid(plist::Uid::new(2)));
        item.insert("_displayName".into(), plist::Value::Uid(plist::Uid::new(3)));
        let mut d = plist::Dictionary::new();
        d.insert("$archiver".into(), "NSKeyedArchiver".into());
        d.insert("$top".into(), plist::Value::Dictionary(top));
        d.insert(
            "$objects".into(),
            plist::Value::Array(vec!["$null".into(), plist::Value::Dictionary(item), "fibonacci heap.txt".into(), "fibonacci heap".into()]),
        );
        let mut buf = Vec::new();
        plist::Value::Dictionary(d).to_writer_binary(&mut buf).unwrap();
        assert_eq!(record_filename(&buf).as_deref(), Some("fibonacci heap.txt"));
    }

    #[test]
    fn text_comes_from_inline_primary_types_only() {
        let reply = |types: &[(&str, Option<&[u8]>)]| {
            let mut data = plist::Dictionary::new();
            for (uti, bytes) in types {
                let mut d = plist::Dictionary::new();
                match bytes {
                    Some(b) => d.insert("data".into(), plist::Value::Data(b.to_vec())),
                    None => d.insert("size".into(), plist::Value::Integer(10.into())),
                };
                data.insert(uti.to_string(), plist::Value::Dictionary(d));
            }
            let mut item = plist::Dictionary::new();
            item.insert("data".into(), plist::Value::Dictionary(data));
            let mut pb = plist::Dictionary::new();
            pb.insert("items".into(), plist::Value::Array(vec![plist::Value::Dictionary(item)]));
            let mut root = plist::Dictionary::new();
            root.insert("pasteboard".into(), plist::Value::Dictionary(pb));
            plist::Value::Dictionary(root)
        };
        assert_eq!(inline_text(&reply(&[("public.utf8-plain-text", Some(b"hi"))])).as_deref(), Some("hi"));
        // Promised (not inline): nothing, rather than a stalling RESOLVE.
        assert_eq!(inline_text(&reply(&[("public.utf8-plain-text", None)])), None);
        assert_eq!(inline_text(&reply(&[("public.rtf", Some(b"{\\rtf1}"))])), None);
    }

    #[test]
    fn pushed_text_is_offered_like_ios_text() {
        let t = push_types(PushContent::Text("héllo".into()));
        assert_eq!(t.iter().map(|(u, _)| *u).collect::<Vec<_>>(), ["public.utf8-plain-text", "public.plain-text", "public.text"]);
        assert!(t.iter().all(|(_, b)| b == "héllo".as_bytes()));
        assert_eq!(push_types(PushContent::Png(vec![1, 2])), vec![("public.png", vec![1, 2])]);
    }

    #[test]
    fn utis() {
        assert_eq!(uti_for(Path::new("a.PNG")), "public.png");
        assert_eq!(uti_for(Path::new("b.pdf")), "com.adobe.pdf");
        assert_eq!(uti_for(Path::new("c")), "public.data");
    }
}
