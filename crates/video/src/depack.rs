//! RTP/HEVC (RFC 7798) depacketizer that closes an access unit on the RTP
//! marker bit, the moment its last packet arrives.
//!
//! idevice's `HevcDepacketizer` instead closes a frame when the *next* frame's
//! first packet shows a new timestamp (up to one frame interval late) and, on
//! loss, waits for 128 more packets before skipping the gap. Here a gap just
//! marks the current access unit corrupt; there is no reorder buffer at all.

const NAL_AP: u8 = 48;
const NAL_FU: u8 = 49;
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// A constant 14-byte footer the displayservice appends after the last
/// fragment of each coded-slice NAL. It is not HEVC; Apple's receiver strips
/// it (fact documented in pymobiledevice3's screen_stream.py). Matched as an
/// exact suffix, so it is a no-op if absent.
const DISPLAYSERVICE_TRAILER: [u8; 14] = [
    0x04, 0xf0, 0x0a, 0xc0, 0x00, 0x00, 0x03, 0x00, 0x00, 0x04, 0xec, 0x0a, 0xb0, 0x03,
];

#[derive(Debug, Clone)]
pub struct AccessUnit {
    /// Annex-B byte stream for one picture (parameter sets included if sent).
    pub data: Vec<u8>,
    pub rtp_ts: u32,
    /// Contains an IRAP picture (NAL types 16..=21).
    pub key: bool,
    /// Contains VPS/SPS/PPS.
    pub params: bool,
    /// A packet was lost or a fragment was broken: do not decode.
    pub corrupt: bool,
    pub packets: u32,
    pub first_pkt_us: u64,
    pub last_pkt_us: u64,
}

#[derive(Debug, Default, Clone)]
pub struct DepackStats {
    pub packets: u64,
    pub gaps: u64,
    pub lost_packets: u64,
    pub late_packets: u64,
    pub aus: u64,
    pub corrupt_aus: u64,
    pub unterminated_aus: u64,
}

#[derive(Default)]
pub struct Depacketizer {
    au: Vec<u8>,
    fu: Vec<u8>,
    fu_open: bool,
    corrupt: bool,
    key: bool,
    params: bool,
    ts: Option<u32>,
    last_seq: Option<u16>,
    packets: u32,
    first_us: u64,
    last_us: u64,
    pub stats: DepackStats,
}

impl Depacketizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one RTP packet's fields; completed access units are appended to
    /// `out`.
    pub fn push(&mut self, seq: u16, timestamp: u32, marker: bool, payload: &[u8], now_us: u64, out: &mut Vec<AccessUnit>) {
        self.stats.packets += 1;
        let mut gap = false;

        if let Some(last) = self.last_seq {
            let delta = seq.wrapping_sub(last);
            if delta == 0 || delta >= 0x8000 {
                // Duplicate or arrived after we moved on: already counted lost.
                self.stats.late_packets += 1;
                return;
            }
            if delta > 1 {
                self.stats.gaps += 1;
                self.stats.lost_packets += (delta - 1) as u64;
                self.corrupt = true;
                self.fu.clear();
                self.fu_open = false;
                gap = true;
            }
        }
        self.last_seq = Some(seq);

        // A new timestamp before we saw a marker: the previous AU's last packet
        // was lost. Flush what we have as corrupt.
        if let Some(ts) = self.ts
            && ts != timestamp
        {
            self.stats.unterminated_aus += 1;
            self.corrupt = true;
            self.finish(out);
            // The lost packet(s) may have belonged to this new AU as well.
            self.corrupt = gap;
        }
        if self.ts.is_none() {
            self.ts = Some(timestamp);
            self.first_us = now_us;
        }
        self.packets += 1;
        self.last_us = now_us;

        self.payload(payload);

        if marker {
            self.finish(out);
        }
    }

    fn payload(&mut self, p: &[u8]) {
        if p.len() < 2 {
            return;
        }
        match (p[0] >> 1) & 0x3f {
            NAL_AP => {
                let mut i = 2;
                while i + 2 <= p.len() {
                    let size = u16::from_be_bytes([p[i], p[i + 1]]) as usize;
                    i += 2;
                    let Some(nal) = p.get(i..i + size) else {
                        self.corrupt = true;
                        return;
                    };
                    self.nal(nal);
                    i += size;
                }
            }
            NAL_FU => {
                if p.len() < 3 {
                    self.corrupt = true;
                    return;
                }
                let h = p[2];
                let (start, end, ty) = (h & 0x80 != 0, h & 0x40 != 0, h & 0x3f);
                if start {
                    self.fu.clear();
                    self.fu.push((p[0] & 0x81) | (ty << 1));
                    self.fu.push(p[1]);
                    self.fu_open = true;
                } else if !self.fu_open {
                    // Middle/end fragment without its start.
                    self.corrupt = true;
                    return;
                }
                self.fu.extend_from_slice(&p[3..]);
                if end {
                    let fu = std::mem::take(&mut self.fu);
                    self.nal(&fu);
                    self.fu = fu;
                    self.fu.clear();
                    self.fu_open = false;
                }
            }
            _ => self.nal(p),
        }
    }

    fn nal(&mut self, nal: &[u8]) {
        let nal = nal.strip_suffix(&DISPLAYSERVICE_TRAILER).unwrap_or(nal);
        if nal.is_empty() {
            return;
        }
        match (nal[0] >> 1) & 0x3f {
            16..=21 => self.key = true,
            32..=34 => self.params = true,
            _ => {}
        }
        self.au.extend_from_slice(&START_CODE);
        self.au.extend_from_slice(nal);
    }

    fn finish(&mut self, out: &mut Vec<AccessUnit>) {
        let Some(ts) = self.ts.take() else { return };
        if self.fu_open {
            self.corrupt = true;
        }
        self.stats.aus += 1;
        if self.corrupt {
            self.stats.corrupt_aus += 1;
        }
        out.push(AccessUnit {
            data: std::mem::take(&mut self.au),
            rtp_ts: ts,
            key: self.key,
            params: self.params,
            corrupt: self.corrupt,
            packets: self.packets,
            first_pkt_us: self.first_us,
            last_pkt_us: self.last_us,
        });
        self.fu.clear();
        self.fu_open = false;
        self.corrupt = false;
        self.key = false;
        self.params = false;
        self.packets = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pkt(u16, u32, bool, Vec<u8>);

    fn rtp(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Pkt {
        Pkt(seq, ts, marker, payload.to_vec())
    }

    fn feed(d: &mut Depacketizer, p: &Pkt, out: &mut Vec<AccessUnit>) {
        d.push(p.0, p.1, p.2, &p.3, 0, out);
    }

    #[test]
    fn single_nal_closes_on_marker() {
        let mut d = Depacketizer::new();
        let mut out = vec![];
        // IDR_W_RADL (type 19): first byte 19<<1 = 0x26
        feed(&mut d, &rtp(1, 10, true, &[0x26, 0x01, 0xAA]), &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].key && !out[0].corrupt);
        assert_eq!(out[0].data, vec![0, 0, 0, 1, 0x26, 0x01, 0xAA]);
    }

    #[test]
    fn fu_reassembly_and_trailer_strip() {
        let mut d = Depacketizer::new();
        let mut out = vec![];
        // FU indicator type 49: 49<<1 = 0x62. FU header: S + type 1.
        feed(&mut d, &rtp(1, 10, false, &[0x62, 0x01, 0x81, 0xBB]), &mut out);
        let mut end = vec![0x62, 0x01, 0x41, 0xCC];
        end.extend_from_slice(&DISPLAYSERVICE_TRAILER);
        feed(&mut d, &rtp(2, 10, true, &end), &mut out);
        assert_eq!(out.len(), 1);
        assert!(!out[0].corrupt && !out[0].key);
        assert_eq!(out[0].data, vec![0, 0, 0, 1, 0x02, 0x01, 0xBB, 0xCC]);
    }

    #[test]
    fn gap_marks_corrupt_and_lost_marker_flushes() {
        let mut d = Depacketizer::new();
        let mut out = vec![];
        feed(&mut d, &rtp(1, 10, false, &[0x02, 0x01, 1]), &mut out);
        // seq 2 (the marker of ts=10) lost; next frame starts.
        feed(&mut d, &rtp(3, 20, true, &[0x02, 0x01, 2]), &mut out);
        assert_eq!(out.len(), 2);
        assert!(out[0].corrupt, "unterminated AU must be corrupt");
        assert!(out[1].corrupt, "AU after the gap is corrupt too");
        assert_eq!(d.stats.lost_packets, 1);
        feed(&mut d, &rtp(4, 30, true, &[0x02, 0x01, 3]), &mut out);
        assert!(!out[2].corrupt);
    }
}
