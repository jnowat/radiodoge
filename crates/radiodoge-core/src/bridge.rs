//! Meshtastic bridge session over a pluggable raw-LoRa link.
//!
//! Firmware FW12 exposes CMD_RAW_TX / CMD_RAW_RX_MODE / CMD_RAW_RX so a
//! RadioDoge host can hear and speak Meshtastic LongFast without changing the
//! board's saved RadioDoge radio settings. The production path talks to a real
//! Heltec over serial; this module also ships an in-process [SimulatedAir] so
//! framing, channel routing, timeouts, and bidirectional RadioDoge↔Meshtastic
//! relay can be exercised with **no hardware**.

use crate::meshtastic::{
    bridge_decode, bridge_text_command, bridge_tx_command, new_packet_id, BridgeEvent, Channel,
};
use crate::radio::{RawProfile, RawRxFrame};
use crate::types::NodeAddress;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Errors raised by a [RawLink] or [BridgeSession].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// The link refused or failed a raw transmit.
    TxFailed(String),
    /// Entering or leaving raw receive mode failed.
    RxModeFailed(String),
    /// The radio is not currently listening (or the listen window expired).
    NotListening,
    /// Channel / frame construction failed.
    Encode(String),
    /// A wait timed out.
    Timeout(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::TxFailed(s) => write!(f, "raw transmit failed: {s}"),
            BridgeError::RxModeFailed(s) => write!(f, "raw receive mode failed: {s}"),
            BridgeError::NotListening => write!(f, "not listening on the Meshtastic channel"),
            BridgeError::Encode(s) => write!(f, "bridge encode: {s}"),
            BridgeError::Timeout(s) => write!(f, "bridge timeout: {s}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// Pluggable raw-LoRa transport used by [BridgeSession].
pub trait RawLink: Send + Sync {
    /// Transmit one already-framed LoRa payload on profile.
    fn raw_tx(&self, profile: &RawProfile, frame: &[u8]) -> Result<(), BridgeError>;

    /// Enter (Some) or leave (None) volatile raw receive.
    fn set_rx_mode(&self, profile: Option<&RawProfile>, timeout_s: u16) -> Result<(), BridgeError>;

    /// Pop one overheard raw frame, if any.
    fn try_recv(&self) -> Option<RawRxFrame>;

    /// Whether raw RX is currently armed and not expired.
    fn is_listening(&self) -> bool;
}

fn profiles_compat(rx: &RawProfile, tx: &RawProfile) -> bool {
    rx.sync_word == tx.sync_word
        && rx.sf == tx.sf
        && rx.bw_idx == tx.bw_idx
        && rx.cr == tx.cr
        && rx.freq_khz == tx.freq_khz
}

/// Shared in-process ether that delivers frames to radios listening on a
/// matching profile (sync/SF/BW/CR/freq).
#[derive(Default)]
pub struct SimulatedAir {
    radios: Mutex<Vec<Arc<SimulatedRadioInner>>>,
}

impl SimulatedAir {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn register(&self, radio: Arc<SimulatedRadioInner>) {
        self.radios.lock().expect("air lock").push(radio);
    }

    fn deliver(&self, from_id: u64, profile: &RawProfile, frame: &[u8]) {
        let snapshot: Vec<_> = self.radios.lock().expect("air lock").clone();
        for r in snapshot {
            if r.id == from_id {
                continue;
            }
            r.deliver_if_listening(profile, frame);
        }
    }
}

struct SignalDefaults {
    rssi: i16,
    snr: i8,
}

struct SimulatedRadioInner {
    id: u64,
    air: Arc<SimulatedAir>,
    fail_tx: AtomicBool,
    refuse_rx_mode: AtomicBool,
    inbox: Mutex<VecDeque<RawRxFrame>>,
    rx: Mutex<Option<(RawProfile, Instant)>>,
    signal: Mutex<SignalDefaults>,
}

impl SimulatedRadioInner {
    fn listening_profile(&self) -> Option<RawProfile> {
        let mut guard = self.rx.lock().expect("rx lock");
        if let Some((p, deadline)) = guard.as_ref() {
            if Instant::now() >= *deadline {
                *guard = None;
                return None;
            }
            return Some(*p);
        }
        None
    }

    fn deliver_if_listening(&self, tx_profile: &RawProfile, frame: &[u8]) {
        let Some(rx_profile) = self.listening_profile() else {
            return;
        };
        if !profiles_compat(&rx_profile, tx_profile) {
            return;
        }
        let sig = self.signal.lock().expect("signal");
        self.inbox.lock().expect("inbox").push_back(RawRxFrame {
            rssi: sig.rssi,
            snr: sig.snr,
            data: frame.to_vec(),
        });
    }
}

static NEXT_RADIO_ID: AtomicU64 = AtomicU64::new(1);

/// One simulated Heltec on a [SimulatedAir].
pub struct SimulatedRadio {
    inner: Arc<SimulatedRadioInner>,
}

impl SimulatedRadio {
    pub fn join(air: &Arc<SimulatedAir>) -> Self {
        let inner = Arc::new(SimulatedRadioInner {
            id: NEXT_RADIO_ID.fetch_add(1, Ordering::Relaxed),
            air: Arc::clone(air),
            fail_tx: AtomicBool::new(false),
            refuse_rx_mode: AtomicBool::new(false),
            inbox: Mutex::new(VecDeque::new()),
            rx: Mutex::new(None),
            signal: Mutex::new(SignalDefaults { rssi: -70, snr: 5 }),
        });
        air.register(Arc::clone(&inner));
        Self { inner }
    }

    /// Next `raw_tx` fails once (simulates board NACK / no reply).
    pub fn fail_next_tx(&self) {
        self.inner.fail_tx.store(true, Ordering::Relaxed);
    }

    pub fn set_refuse_rx_mode(&self, refuse: bool) {
        self.inner.refuse_rx_mode.store(refuse, Ordering::Relaxed);
    }

    pub fn set_signal(&self, rssi: i16, snr: i8) {
        let mut s = self.inner.signal.lock().expect("signal");
        s.rssi = rssi;
        s.snr = snr;
    }
}

impl RawLink for SimulatedRadio {
    fn raw_tx(&self, profile: &RawProfile, frame: &[u8]) -> Result<(), BridgeError> {
        if frame.is_empty() {
            return Err(BridgeError::TxFailed("empty frame".into()));
        }
        if self.inner.fail_tx.swap(false, Ordering::Relaxed) {
            return Err(BridgeError::TxFailed("simulated NACK".into()));
        }
        profile.validate().map_err(BridgeError::TxFailed)?;
        self.inner.air.deliver(self.inner.id, profile, frame);
        Ok(())
    }

    fn set_rx_mode(&self, profile: Option<&RawProfile>, timeout_s: u16) -> Result<(), BridgeError> {
        match profile {
            Some(p) => {
                if self.inner.refuse_rx_mode.load(Ordering::Relaxed) {
                    return Err(BridgeError::RxModeFailed("simulated refuse".into()));
                }
                p.validate().map_err(BridgeError::RxModeFailed)?;
                let secs = timeout_s.max(1) as u64;
                *self.inner.rx.lock().expect("rx") =
                    Some((*p, Instant::now() + Duration::from_secs(secs)));
            }
            None => {
                *self.inner.rx.lock().expect("rx") = None;
            }
        }
        Ok(())
    }

    fn try_recv(&self) -> Option<RawRxFrame> {
        let _ = self.inner.listening_profile(); // expire if due
        self.inner.inbox.lock().expect("inbox").pop_front()
    }

    fn is_listening(&self) -> bool {
        self.inner.listening_profile().is_some()
    }
}

/// High-level Meshtastic bridge on top of any [RawLink].
pub struct BridgeSession<L: RawLink> {
    link: L,
    host: NodeAddress,
    from: u32,
    channel: Channel,
    profile: RawProfile,
}

impl<L: RawLink> BridgeSession<L> {
    pub fn new(
        link: L,
        host: NodeAddress,
        from: u32,
        channel: Channel,
        profile: RawProfile,
    ) -> Result<Self, BridgeError> {
        profile.validate().map_err(BridgeError::Encode)?;
        Ok(Self {
            link,
            host,
            from,
            channel,
            profile,
        })
    }

    pub fn link(&self) -> &L {
        &self.link
    }

    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    pub fn profile(&self) -> &RawProfile {
        &self.profile
    }

    /// Enter raw RX for timeout_s seconds.
    pub fn listen(&self, timeout_s: u16) -> Result<(), BridgeError> {
        self.link.set_rx_mode(Some(&self.profile), timeout_s)
    }

    /// Leave raw RX.
    pub fn stop_listen(&self) -> Result<(), BridgeError> {
        self.link.set_rx_mode(None, 0)
    }

    pub fn is_listening(&self) -> bool {
        self.link.is_listening()
    }

    /// Wrap and transmit a RadioDoge app packet (PRIVATE_APP).
    pub fn send_radiodoge(&self, packet: &[u8]) -> Result<u32, BridgeError> {
        let id = new_packet_id();
        let cmd = bridge_tx_command(
            &self.host,
            self.from,
            id,
            packet,
            &self.channel,
            &self.profile,
        )
        .map_err(BridgeError::Encode)?;
        // cmd = header(8) + profile + frame
        let frame = extract_raw_frame(&cmd)?;
        self.link.raw_tx(&self.profile, frame)?;
        Ok(id)
    }

    /// Transmit a Meshtastic text message.
    pub fn send_text(&self, text: &str) -> Result<u32, BridgeError> {
        let id = new_packet_id();
        let cmd = bridge_text_command(
            &self.host,
            self.from,
            id,
            text,
            &self.channel,
            &self.profile,
        )
        .map_err(BridgeError::Encode)?;
        let frame = extract_raw_frame(&cmd)?;
        self.link.raw_tx(&self.profile, frame)?;
        Ok(id)
    }

    /// Decode the next overheard frame, if any.
    pub fn try_recv_event(&self) -> Option<(BridgeEvent, RawRxFrame)> {
        let frame = self.link.try_recv()?;
        let ev = bridge_decode(&frame.data, &self.channel);
        Some((ev, frame))
    }

    /// Block-poll until an event, listen expiry, or timeout.
    pub fn recv_event_until(
        &self,
        timeout: Duration,
    ) -> Result<(BridgeEvent, RawRxFrame), BridgeError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(ev) = self.try_recv_event() {
                return Ok(ev);
            }
            if !self.link.is_listening() {
                return Err(BridgeError::NotListening);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Err(BridgeError::Timeout("no Meshtastic frame".into()))
    }
}

fn extract_raw_frame(cmd: &[u8]) -> Result<&[u8], BridgeError> {
    use crate::radio::RAW_PROFILE_LEN;
    let start = 8 + RAW_PROFILE_LEN;
    if cmd.len() <= start {
        return Err(BridgeError::Encode("raw TX command has no frame".into()));
    }
    Ok(&cmd[start..])
}

/// Bidirectional RadioDoge ↔ Meshtastic relay helper.
///
/// - Mesh BridgeEvent::RadioDoge payloads are queued for the local app/daemon.
/// - [BridgeRelay::forward_to_mesh] wraps an app packet and TX's it on the channel.
pub struct BridgeRelay<L: RawLink> {
    session: BridgeSession<L>,
    inbound_app: Mutex<VecDeque<RelayedAppPacket>>,
}

/// A RadioDoge app packet that arrived via Meshtastic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayedAppPacket {
    pub from_mesh_node: u32,
    pub packet: Vec<u8>,
    pub rssi: i16,
    pub snr: i8,
}

impl<L: RawLink> BridgeRelay<L> {
    pub fn new(session: BridgeSession<L>) -> Self {
        Self {
            session,
            inbound_app: Mutex::new(VecDeque::new()),
        }
    }

    pub fn session(&self) -> &BridgeSession<L> {
        &self.session
    }

    /// Poll the link; RadioDoge mesh events are queued, others returned.
    pub fn poll(&self) -> Option<BridgeEvent> {
        let (ev, meta) = self.session.try_recv_event()?;
        if let BridgeEvent::RadioDoge { from, packet } = &ev {
            self.inbound_app
                .lock()
                .expect("inbound")
                .push_back(RelayedAppPacket {
                    from_mesh_node: *from,
                    packet: packet.clone(),
                    rssi: meta.rssi,
                    snr: meta.snr,
                });
        }
        Some(ev)
    }

    pub fn take_app_packet(&self) -> Option<RelayedAppPacket> {
        self.inbound_app.lock().expect("inbound").pop_front()
    }

    pub fn forward_to_mesh(&self, packet: &[u8]) -> Result<u32, BridgeError> {
        self.session.send_radiodoge(packet)
    }

    pub fn listen(&self, timeout_s: u16) -> Result<(), BridgeError> {
        self.session.listen(timeout_s)
    }

    pub fn stop_listen(&self) -> Result<(), BridgeError> {
        self.session.stop_listen()
    }
}

/// Build a pair of bridged simulated radios on LongFast for tests / CLI sim.
pub fn paired_longfast_relays(
    node_a: u32,
    node_b: u32,
) -> (BridgeRelay<SimulatedRadio>, BridgeRelay<SimulatedRadio>) {
    let air = SimulatedAir::new();
    let host = NodeAddress::new(10, 0, 1);
    let ch = Channel::default_longfast();
    let profile = RawProfile::meshtastic_longfast_us();
    let a = BridgeRelay::new(
        BridgeSession::new(
            SimulatedRadio::join(&air),
            host.clone(),
            node_a,
            ch.clone(),
            profile,
        )
        .unwrap(),
    );
    let b = BridgeRelay::new(
        BridgeSession::new(
            SimulatedRadio::join(&air),
            host.clone(),
            node_b,
            ch,
            profile,
        )
        .unwrap(),
    );
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::{self, CMD_PING};

    #[test]
    fn simulated_peers_exchange_text_and_radiodoge() {
        let air = SimulatedAir::new();
        let host = NodeAddress::new(10, 0, 1);
        let ch = Channel::default_longfast();
        let profile = RawProfile::meshtastic_longfast_us();
        let a = BridgeSession::new(
            SimulatedRadio::join(&air),
            host.clone(),
            0x1111,
            ch.clone(),
            profile,
        )
        .unwrap();
        let b = BridgeSession::new(
            SimulatedRadio::join(&air),
            host.clone(),
            0x2222,
            ch,
            profile,
        )
        .unwrap();
        b.listen(5).unwrap();
        a.send_text("much wow").unwrap();
        let (ev, meta) = b.recv_event_until(Duration::from_secs(1)).unwrap();
        assert_eq!(
            ev,
            BridgeEvent::Text {
                from: 0x1111,
                text: "much wow".into()
            }
        );
        assert_eq!(meta.rssi, -70);
        a.listen(5).unwrap();
        let doge = [CMD_PING, 0, 10, 0, 1, 10, 0, 2];
        b.send_radiodoge(&doge).unwrap();
        let (ev, _) = a.recv_event_until(Duration::from_secs(1)).unwrap();
        assert_eq!(
            ev,
            BridgeEvent::RadioDoge {
                from: 0x2222,
                packet: doge.to_vec()
            }
        );
    }

    #[test]
    fn wrong_channel_is_undecodable_not_delivered_as_app() {
        let air = SimulatedAir::new();
        let host = NodeAddress::new(1, 1, 1);
        let longfast = Channel::default_longfast();
        let other = Channel {
            name: "Other".into(),
            key: vec![1],
        };
        let profile = RawProfile::meshtastic_longfast_us();
        let a = BridgeSession::new(
            SimulatedRadio::join(&air),
            host.clone(),
            1,
            longfast,
            profile,
        )
        .unwrap();
        let b = BridgeSession::new(SimulatedRadio::join(&air), host, 2, other, profile).unwrap();
        b.listen(5).unwrap();
        a.send_text("secret").unwrap();
        let (ev, _) = b.recv_event_until(Duration::from_secs(1)).unwrap();
        assert_eq!(ev, BridgeEvent::Undecodable);
    }

    #[test]
    fn profile_mismatch_is_not_heard() {
        let air = SimulatedAir::new();
        let host = NodeAddress::new(1, 1, 1);
        let ch = Channel::default_longfast();
        let p1 = RawProfile::meshtastic_longfast_us();
        let mut p2 = p1;
        p2.freq_khz = 904_375;
        let a = BridgeSession::new(SimulatedRadio::join(&air), host.clone(), 1, ch.clone(), p1)
            .unwrap();
        let b = BridgeSession::new(SimulatedRadio::join(&air), host.clone(), 2, ch, p2).unwrap();
        b.listen(5).unwrap();
        a.send_text("miss").unwrap();
        assert!(b.try_recv_event().is_none());
        assert!(b.is_listening());
    }

    #[test]
    fn tx_failure_and_rx_mode_refusal() {
        let air = SimulatedAir::new();
        let host = NodeAddress::new(1, 1, 1);
        let ch = Channel::default_longfast();
        let profile = RawProfile::meshtastic_longfast_us();
        let radio = SimulatedRadio::join(&air);
        radio.fail_next_tx();
        let s = BridgeSession::new(radio, host.clone(), 1, ch.clone(), profile).unwrap();
        assert!(matches!(s.send_text("x"), Err(BridgeError::TxFailed(_))));
        let radio2 = SimulatedRadio::join(&air);
        radio2.set_refuse_rx_mode(true);
        let s2 = BridgeSession::new(radio2, host.clone(), 2, ch, profile).unwrap();
        assert!(matches!(s2.listen(3), Err(BridgeError::RxModeFailed(_))));
    }

    #[test]
    fn listen_timeout_expires() {
        let air = SimulatedAir::new();
        let host = NodeAddress::new(1, 1, 1);
        let s = BridgeSession::new(
            SimulatedRadio::join(&air),
            host.clone(),
            1,
            Channel::default_longfast(),
            RawProfile::meshtastic_longfast_us(),
        )
        .unwrap();
        s.listen(1).unwrap();
        assert!(s.is_listening());
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!s.is_listening());
        assert_eq!(
            s.recv_event_until(Duration::from_millis(50)),
            Err(BridgeError::NotListening)
        );
    }

    #[test]
    fn relay_queues_app_packets_both_ways() {
        let (a, b) = paired_longfast_relays(0xA, 0xB);
        a.listen(5).unwrap();
        b.listen(5).unwrap();
        let ping = radio::build_ping(&NodeAddress::new(10, 0, 1), &NodeAddress::new(10, 0, 2));
        a.forward_to_mesh(&ping).unwrap();
        // drain non-app? should be RadioDoge
        let mut saw = None;
        for _ in 0..50 {
            if let Some(ev) = b.poll() {
                if matches!(ev, BridgeEvent::RadioDoge { .. }) {
                    saw = b.take_app_packet();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let got = saw.expect("relayed app packet");
        assert_eq!(got.from_mesh_node, 0xA);
        assert_eq!(got.packet, ping);
        // reply
        let reply = radio::build_ping(&NodeAddress::new(10, 0, 2), &NodeAddress::new(10, 0, 1));
        b.forward_to_mesh(&reply).unwrap();
        let mut saw = None;
        for _ in 0..50 {
            let _ = a.poll();
            if let Some(p) = a.take_app_packet() {
                saw = Some(p);
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(saw.unwrap().packet, reply);
    }

    #[test]
    fn stop_listen_blocks_further_rx() {
        let air = SimulatedAir::new();
        let host = NodeAddress::new(1, 1, 1);
        let ch = Channel::default_longfast();
        let profile = RawProfile::meshtastic_longfast_us();
        let a = BridgeSession::new(
            SimulatedRadio::join(&air),
            host.clone(),
            1,
            ch.clone(),
            profile,
        )
        .unwrap();
        let b =
            BridgeSession::new(SimulatedRadio::join(&air), host.clone(), 2, ch, profile).unwrap();
        b.listen(10).unwrap();
        b.stop_listen().unwrap();
        a.send_text("gone").unwrap();
        assert!(b.try_recv_event().is_none());
    }
}
