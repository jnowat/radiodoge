//! Meshtastic subcommands: bridge and channel tools.

use super::*;

pub(crate) fn parse_node_num(from: &str) -> Result<u32> {
    match from.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16),
        None => from.parse(),
    }
    .context("--from must be a u32")
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_meshtastic_bridge(
    port: &str,
    listen: u16,
    send_hex: Option<String>,
    text: Option<String>,
    from: &str,
    freq_khz: u32,
    power: i8,
    channel: ChannelArgs,
) -> Result<()> {
    use radiodoge_core::meshtastic as mt;
    let ch = channel.to_channel()?;
    let from = parse_node_num(from)?;
    let profile = radio::RawProfile {
        freq_khz,
        power_dbm: power,
        ..radio::RawProfile::meshtastic_longfast_us()
    };
    profile.validate().map_err(|e| anyhow::anyhow!(e))?;

    let manager = Arc::new(SerialManager::new());
    let on_packet = Arc::new(|_: IncomingPacket| {});
    manager
        .connect(port, on_packet)
        .await
        .with_context(|| format!("Failed to open serial port {}", port))?;
    let fw = manager.ensure_firmware_version().await;
    if !radio::firmware_supports_raw(fw.as_deref()) {
        manager.disconnect().await.ok();
        anyhow::bail!("Bridge mode needs firmware FW{} or newer; the board reports {}. Flash heltec-firmware-v3/.",
            radio::MIN_RAW_LORA_FIRMWARE, fw.as_deref().unwrap_or("no version"));
    }
    let host = manager.get_node_address().await;
    let result = async {
        let send = match (send_hex, text) {
            (Some(h), _) => Some(mt::bridge_tx_command(
                &host,
                from,
                mt::new_packet_id(),
                &hex::decode(h.trim()).context("--send-hex must be hex")?,
                &ch,
                &profile,
            )),
            (None, Some(t)) => Some(mt::bridge_text_command(
                &host,
                from,
                mt::new_packet_id(),
                &t,
                &ch,
                &profile,
            )),
            _ => None,
        };
        if let Some(cmd) = send {
            let cmd = cmd.map_err(|e| anyhow::anyhow!(e))?;
            match manager
                .send_and_get_reply(cmd, radio::CMD_RAW_TX, 8000)
                .await?
            {
                Some(r) if r.payload_hex == "01" => {
                    println!("📡 Sent on {} ({} kHz).", ch.name, freq_khz)
                }
                Some(_) => println!("❌ The board reported the raw transmit failed."),
                None => {
                    println!("❌ No reply to the raw transmit (rejected profile or old firmware).")
                }
            }
        }
        if listen == 0 {
            return Ok(());
        }
        let mut rx = manager.subscribe_packets();
        let on = radio::build_raw_rx_mode(&host, Some(&profile), listen.saturating_add(5))
            .map_err(|e| anyhow::anyhow!(e))?;
        match manager
            .send_and_get_reply(on, radio::CMD_RAW_RX_MODE, 3000)
            .await?
        {
            Some(r) if r.payload_hex == "01" => {}
            _ => anyhow::bail!("The board did not enter raw receive mode."),
        }
        println!(
            "👂 Listening on Meshtastic {} for {} s (Ctrl+C to stop)...",
            ch.name, listen
        );
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(listen as u64);
        loop {
            let pkt = tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                _ = tokio::signal::ctrl_c() => break,
                p = rx.recv() => match p { Ok(p) => p, Err(_) => continue },
            };
            if pkt.command != radio::CMD_RAW_RX {
                continue;
            }
            let Some(f) = hex::decode(&pkt.payload_hex)
                .ok()
                .and_then(|b| radio::parse_raw_rx(&b))
            else {
                continue;
            };
            let sig = format!("rssi={} snr={}", f.rssi, f.snr);
            match mt::bridge_decode(&f.data, &ch) {
                mt::BridgeEvent::RadioDoge { from, packet } => println!(
                    "🐕 RadioDoge from !{:08x} {}: {}",
                    from,
                    sig,
                    hex::encode(packet)
                ),
                mt::BridgeEvent::Text { from, text } => {
                    println!("💬 !{:08x} {}: {}", from, sig, text)
                }
                mt::BridgeEvent::Other { from, portnum } => {
                    println!("·  !{:08x} {} portnum {}", from, sig, portnum)
                }
                mt::BridgeEvent::Undecodable => {
                    println!("·  {} B not on this channel {}", f.data.len(), sig)
                }
            }
        }
        Ok(())
    }
    .await;
    // Always leave raw mode, even after an error.
    if let Ok(off) = radio::build_raw_rx_mode(&host, None, 0) {
        let _ = manager
            .send_and_get_reply(off, radio::CMD_RAW_RX_MODE, 2000)
            .await;
    }
    manager.disconnect().await.ok();
    result
}

pub(crate) fn cmd_meshtastic_sim(rounds: u32, text: Option<String>) -> Result<()> {
    use radiodoge_core::bridge::paired_longfast_relays;
    use radiodoge_core::meshtastic::BridgeEvent;
    use radiodoge_core::radio;
    use radiodoge_core::types::NodeAddress;

    let rounds = rounds.clamp(1, 20);
    let (a, b) = paired_longfast_relays(0x5244_0001, 0x5244_0002);
    a.listen(30).map_err(|e| anyhow::anyhow!("{e}"))?;
    b.listen(30).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("🐕 Meshtastic bridge simulator (no hardware)");
    println!(
        "   LongFast peers !52440001 <-> !52440002, {} round(s)",
        rounds
    );

    let src = NodeAddress::new(10, 0, 1);
    let dst = NodeAddress::new(10, 0, 2);
    for i in 1..=rounds {
        let ping = radio::build_ping(&src, &dst);
        a.forward_to_mesh(&ping)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut got = None;
        for _ in 0..200 {
            let _ = b.poll();
            if let Some(p) = b.take_app_packet() {
                got = Some(p);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let p = got.ok_or_else(|| anyhow::anyhow!("sim: peer B missed RadioDoge ping"))?;
        println!(
            "  [{i}] PRIVATE_APP ping  {} B  rssi={} snr={}  ok",
            p.packet.len(),
            p.rssi,
            p.snr
        );

        let msg = text.clone().unwrap_or_else(|| format!("sim-round-{i}"));
        b.session()
            .send_text(&msg)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut saw = false;
        for _ in 0..200 {
            if let Some(ev) = a.poll() {
                if matches!(ev, BridgeEvent::Text { .. }) {
                    saw = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if !saw {
            anyhow::bail!("sim: peer A missed text reply");
        }
        println!("  [{i}] text reply ok: {msg}");
    }
    a.stop_listen().ok();
    b.stop_listen().ok();
    println!("✅ Simulator finished {rounds} round(s) with no hardware.");
    Ok(())
}

pub(crate) fn cmd_meshtastic(cmd: MeshtasticCommands) -> Result<()> {
    use radiodoge_core::meshtastic as mt;
    match cmd {
        MeshtasticCommands::Bridge { .. } => unreachable!("bridge is async; dispatched in main"),
        MeshtasticCommands::Sim { rounds, text } => return cmd_meshtastic_sim(rounds, text),
        MeshtasticCommands::Wrap {
            packet_hex,
            from,
            id,
            channel,
        } => {
            let pkt = hex::decode(packet_hex.trim()).context("packet must be hex")?;
            let from = match from.strip_prefix("0x") {
                Some(h) => u32::from_str_radix(h, 16),
                None => from.parse(),
            }
            .context("--from must be a u32")?;
            let id = id.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
                    .unwrap_or(1)
            });
            let frame = mt::wrap_radiodoge(from, id, &pkt, &channel.to_channel()?)?;
            println!("{}", hex::encode(frame));
        }
        MeshtasticCommands::Decode { frame_hex, channel } => {
            let frame = hex::decode(frame_hex.trim()).context("frame must be hex")?;
            match mt::decode_frame(&frame, &channel.to_channel()?)? {
                None => println!(
                    "Channel hash 0x{:02x} doesn't match this channel.",
                    frame.get(13).copied().unwrap_or(0)
                ),
                Some((h, d)) => {
                    println!(
                        "to=0x{:08x} from=0x{:08x} id={} hops={}/{} want_ack={} channel=0x{:02x}",
                        h.to, h.from, h.id, h.hop_limit, h.hop_start, h.want_ack, h.channel_hash
                    );
                    println!("portnum={} payload={}", d.portnum, hex::encode(&d.payload));
                    if d.portnum == mt::PORT_TEXT_MESSAGE {
                        println!("text: {}", String::from_utf8_lossy(&d.payload));
                    }
                }
            }
        }
    }
    Ok(())
}
