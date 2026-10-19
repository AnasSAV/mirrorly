//! Start the displayservice screen-sharing session (audio first, then video on
//! the same clientSessionID, as Device Hub does) and pull the RTCP parameters
//! out of the device's answer.

use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use idevice::{
    ReadWrite, RsdService,
    core_device::{
        CallInfoBlob, DisplayServiceClient, build_screen_audio_offer, build_screen_video_offer,
        build_start_audio_parameters, build_start_video_parameters,
    },
    tcp::handle::UdpSocketHandle,
};

use crate::conn::{Tunnel, ms};

/// What *we* support, not the device's mask (value from Device Hub captures,
/// via idevice's tools).
const CLIENT_SUPPORTED_FEATURES: u64 = 140;

pub struct MediaSession {
    pub client: DisplayServiceClient<Box<dyn ReadWrite>>,
    pub video: Arc<UdpSocketHandle>,
    _audio_drain: tokio::task::JoinHandle<()>,
    /// SSRC we put in the offer; the answer's `RemoteSSRC` overrides it.
    pub our_ssrc: u32,
    /// The device's media SSRC (`LocalSSRC` in its answer).
    pub device_ssrc: Option<u32>,
    /// Device UDP port that receives our RTCP (`SourcePort`).
    pub rtcp_port: Option<u16>,
    pub answer: plist::Value,
    pub start_ms: f64,
}

pub async fn start(t: &mut Tunnel, display_id: i64) -> Result<MediaSession> {
    let started = Instant::now();
    let mut client = DisplayServiceClient::connect_rsd(&mut t.adapter, &mut t.handshake)
        .await
        .context("connect com.apple.coredevice.displayservice (is the DDI installed?)")?;

    let audio_udp = t.adapter.bind_udp(0).await?;
    let video_udp = t.adapter.bind_udp(0).await?;

    let call_info = CallInfoBlob {
        call_id: 0,
        client_version: 1,
        device_type: "Mac17,7".into(),
        framework_version: "2205.3.1".into(),
        os_version: "25F71".into(),
        device_name: None,
        audio_device_uid: None,
    };
    let session_id = uuid::Uuid::new_v4();

    let audio_call = uuid::Uuid::new_v4().to_string().to_uppercase();
    let audio_offer = build_screen_audio_offer(&audio_call, &call_info)?;
    client
        .start_media_stream(build_start_audio_parameters(
            &t.host_ip,
            audio_udp.local_port(),
            &t.device_ip,
            50000,
            audio_offer,
            CLIENT_SUPPORTED_FEATURES,
            session_id,
        ))
        .await
        .map_err(readable)
        .context("audio startmediastream")?;

    let video_call = uuid::Uuid::new_v4().to_string().to_uppercase();
    let our_ssrc = uuid::Uuid::new_v4().as_u128() as u32;
    let video_offer = build_screen_video_offer(&video_call, &call_info, our_ssrc)?;
    let answer = client
        .start_media_stream(build_start_video_parameters(
            &t.host_ip,
            video_udp.local_port(),
            &t.device_ip,
            50001,
            video_offer,
            CLIENT_SUPPORTED_FEATURES,
            display_id,
            session_id,
        ))
        .await
        .map_err(readable)
        .context("video startmediastream")?;

    // Names in streamConfig are from the device's point of view.
    let our_ssrc = find_int(&answer, "RemoteSSRC").map(|v| v as u32).unwrap_or(our_ssrc);
    let device_ssrc = find_int(&answer, "LocalSSRC").map(|v| v as u32);
    let rtcp_port = find_int(&answer, "SourcePort").map(|v| v as u16);

    // Nobody reads the audio; drain it so jktcp's unbounded queue can't grow.
    let audio_drain = tokio::spawn(async move { while audio_udp.recv().await.is_ok() {} });

    Ok(MediaSession {
        client,
        video: Arc::new(video_udp),
        _audio_drain: audio_drain,
        our_ssrc,
        device_ssrc,
        rtcp_port,
        answer,
        start_ms: ms(started),
    })
}

/// Depth-first search for an integer under `key`, accepting plain integers and
/// Swift-codable `{"int": n}` / `{"uint": n}` wrappers.
pub fn find_int(v: &plist::Value, key: &str) -> Option<i64> {
    match v {
        plist::Value::Dictionary(d) => {
            if let Some(found) = d.get(key).and_then(as_int) {
                return Some(found);
            }
            d.values().find_map(|c| find_int(c, key))
        }
        plist::Value::Array(a) => a.iter().find_map(|c| find_int(c, key)),
        _ => None,
    }
}

fn as_int(v: &plist::Value) -> Option<i64> {
    match v {
        plist::Value::Integer(i) => i.as_signed().or(i.as_unsigned().map(|u| u as i64)),
        plist::Value::String(s) => s.parse().ok(),
        plist::Value::Dictionary(d) => d.get("int").or(d.get("uint")).and_then(as_int),
        _ => None,
    }
}

/// A CoreDevice error as its message ("Remote control requires iOS 27.0 or
/// later on this device.") instead of the whole reply with its archived
/// user info.
fn readable(e: idevice::IdeviceError) -> anyhow::Error {
    let text = e.to_string();
    let key = "\"NSLocalizedDescription\": String(\"";
    match text.find(key).map(|i| &text[i + key.len()..]).and_then(|rest| rest.split_once("\")")) {
        Some((msg, _)) => anyhow::anyhow!("{msg}"),
        None => anyhow::Error::new(e),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn device_errors_read_as_their_message() {
        // Shape of the iPadOS 26.1 reply (archive bytes trimmed).
        let mut info = plist::Dictionary::new();
        info.insert("NSLocalizedDescription".into(), "Remote control requires iOS 27.0 or later on this device.".into());
        let mut d = plist::Dictionary::new();
        d.insert("code".into(), 9021.into());
        d.insert("userInfo".into(), plist::Value::Dictionary(info));
        let e = idevice::IdeviceError::UnexpectedResponse(format!("{:?}", plist::Value::Dictionary(d)));
        assert_eq!(super::readable(e).to_string(), "Remote control requires iOS 27.0 or later on this device.");
        let plain = super::readable(idevice::IdeviceError::UnexpectedResponse("odd".into()));
        assert!(plain.to_string().contains("odd"));
    }
}
