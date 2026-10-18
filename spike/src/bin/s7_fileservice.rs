//! s7: can the CoreDevice file service reach the Files app's "On My iPhone"
//! storage? Read-only: opens sessions on candidate domains and lists their
//! top level (entry counts; names only for the container's own top level).
//!
//!   s7_fileservice [--domain 2 --id group.x --path "File Provider Storage"]

use anyhow::Result;
use clap::Parser;
use idevice::{
    RsdService,
    core_device::{Domain, FileServiceClient},
};
use spike::conn::{NoDelayUsbmux, open_tunnel, usb_device};

#[derive(Parser)]
struct Args {
    /// Only this domain (1 app data, 2 app group, 3 temporary).
    #[arg(long)]
    domain: Option<u64>,
    #[arg(long)]
    id: Option<String>,
    /// Also count entries under this path.
    #[arg(long)]
    path: Option<String>,
}

fn domain(n: u64) -> Domain {
    match n {
        1 => Domain::AppDataContainer,
        2 => Domain::AppGroupDataContainer,
        5 => Domain::SystemCrashLogs,
        _ => Domain::Temporary,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    spike::init_logging();
    let args = Args::parse();
    let dev = usb_device().await?;
    let provider = NoDelayUsbmux::new(&dev);
    let mut t = open_tunnel(&provider).await?;

    let candidates: Vec<(u64, String)> = match (args.domain, &args.id) {
        (Some(d), Some(id)) => vec![(d, id.clone())],
        _ => vec![
            (2, "group.com.apple.FileProvider.LocalStorage".into()),
            (2, "group.com.apple.DocumentsApp".into()),
            (1, "com.apple.DocumentsApp".into()),
            (1, "org.videolan.vlc-ios".into()),
            (3, String::new()),
        ],
    };
    for (d, id) in candidates {
        let mut fs = FileServiceClient::connect_rsd(&mut t.adapter, &mut t.handshake).await?;
        print!("domain {d} {id:?}: ");
        match fs.create_session(domain(d), &id).await {
            Err(e) => println!("session refused: {e}"),
            Ok(_) => match fs.retrieve_directory_list(".").await {
                Err(e) => println!("session ok, list refused: {e}"),
                Ok(list) => {
                    println!("{} entries: {:?}", list.len(), list.iter().take(12).collect::<Vec<_>>());
                    if let Some(p) = &args.path {
                        match fs.retrieve_directory_list(p).await {
                            Ok(l) => println!("  {p}: {} entries", l.len()),
                            Err(e) => println!("  {p}: {e}"),
                        }
                    }
                }
            },
        }
    }
    Ok(())
}
