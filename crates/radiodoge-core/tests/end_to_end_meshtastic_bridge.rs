//! End-to-end Meshtastic bridge scenarios over the in-process simulator.
//!
//! No hardware: two `SimulatedRadio` peers share a `SimulatedAir`. Frames still
//! go through the real Meshtastic wrap/decode and RadioDoge packet builders so
//! a protocol regression fails this suite the same way a board test would.

use radiodoge_core::bridge::{
    paired_longfast_relays, BridgeError, BridgeSession, SimulatedAir, SimulatedRadio,
};
use radiodoge_core::meshtastic::{BridgeEvent, Channel};
use radiodoge_core::radio::{self, RawProfile, CMD_PING, CMD_REQUEST_BALANCE};
use radiodoge_core::types::NodeAddress;
use std::time::Duration;

fn drain_app(
    relay: &radiodoge_core::bridge::BridgeRelay<SimulatedRadio>,
    max_ms: u64,
) -> radiodoge_core::bridge::RelayedAppPacket {
    let steps = (max_ms / 2).max(1);
    for _ in 0..steps {
        let _ = relay.poll();
        if let Some(p) = relay.take_app_packet() {
            return p;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("expected relayed RadioDoge app packet within {max_ms} ms");
}

#[test]
fn e2e_ping_over_meshtastic_private_app() {
    let (a, b) = paired_longfast_relays(0x5244_0001, 0x5244_0002);
    a.listen(10).unwrap();
    b.listen(10).unwrap();
    let src = NodeAddress::new(10, 0, 1);
    let dst = NodeAddress::new(10, 0, 2);
    let ping = radio::build_ping(&src, &dst);
    assert_eq!(ping[0], CMD_PING);
    a.forward_to_mesh(&ping).unwrap();
    let got = drain_app(&b, 1000);
    assert_eq!(got.from_mesh_node, 0x5244_0001);
    assert_eq!(got.packet, ping);
    assert!(got.rssi < 0);
}

#[test]
fn e2e_balance_request_bytes_survive_mesh_wrap() {
    let (a, b) = paired_longfast_relays(1, 2);
    a.listen(10).unwrap();
    b.listen(10).unwrap();
    let src = NodeAddress::new(10, 0, 1);
    let gw = NodeAddress::new(10, 0, 9);
    let doge = "DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L";
    let frame = radio::build_request_balance_to(&src, &gw, doge);
    assert_eq!(frame[0], CMD_REQUEST_BALANCE);
    a.forward_to_mesh(&frame).unwrap();
    let got = drain_app(&b, 1000);
    assert_eq!(got.packet, frame);
    assert!(got.packet.windows(doge.len()).any(|w| w == doge.as_bytes()));
}

#[test]
fn e2e_listen_timeout_then_misses_traffic() {
    let air = SimulatedAir::new();
    let host = NodeAddress::new(10, 0, 1);
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
    let b = BridgeSession::new(SimulatedRadio::join(&air), host, 2, ch, profile).unwrap();
    b.listen(1).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    assert!(!b.is_listening());
    a.send_text("too late").unwrap();
    assert!(b.try_recv_event().is_none());
    assert_eq!(
        b.recv_event_until(Duration::from_millis(30)),
        Err(BridgeError::NotListening)
    );
}

#[test]
fn e2e_text_and_other_port_do_not_queue_as_app() {
    let (a, b) = paired_longfast_relays(3, 4);
    a.listen(10).unwrap();
    b.listen(10).unwrap();
    a.session().send_text("hello mesh").unwrap();
    let mut saw_text = false;
    for _ in 0..50 {
        if let Some(ev) = b.poll() {
            if matches!(ev, BridgeEvent::Text { .. }) {
                saw_text = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(saw_text);
    assert!(b.take_app_packet().is_none());
}
