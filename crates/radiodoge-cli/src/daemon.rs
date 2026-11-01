//! Headless gateway daemon (`radiodoge-cli daemon`): broadcast relay, outbox, stats file.

use super::*;

/// Headless daemon — replaces serdog (C serial daemon).
///
/// Connects to the device, logs all received packets indefinitely.
/// When a signed Dogecoin transaction is received (CMD_DOGE_TX with a raw
/// transaction payload), broadcasts it to the Dogecoin network via the
/// configured chain backends and sends a TX_ACK message back to the originator.
///
/// Multipart sequences are reassembled by the serial read loop before this
/// callback runs, so a transaction that arrived split across several frames is
/// delivered here as one complete payload — no reassembly needed at this layer.
///
/// Designed for Raspberry Pi gateway deployments.
/// Shared, lock-protected gateway counters. A std mutex: every critical section
/// is a few field updates and never awaits.
pub(crate) type SharedStats = Arc<std::sync::Mutex<GatewayStats>>;
pub(crate) type SharedOutbox = Arc<std::sync::Mutex<Outbox>>;
/// Transactions already broadcast this run: local txid → network txid.
pub(crate) type SeenTxs = Arc<tokio::sync::Mutex<std::collections::HashMap<String, String>>>;

/// Write `contents` to `path` through a temporary file so a reader never sees half a file.
pub(crate) fn write_atomic(path: &std::path::Path, contents: &str) {
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, contents).and_then(|_| std::fs::rename(&tmp, path)) {
        log::warn!("Could not write {}: {}", path.display(), e);
    }
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn stats_record(
    stats: &SharedStats,
    kind: GatewayEventKind,
    node: &NodeAddress,
    detail: impl Into<String>,
) {
    if let Ok(mut s) = stats.lock() {
        s.record(kind, &node.to_display_string(), detail, unix_now());
    }
}

/// Write the snapshot next to its destination and rename it into place, so a
/// reader never sees half a file.
pub(crate) fn write_stats_file(path: &std::path::Path, stats: &SharedStats) {
    let json = match stats.lock() {
        Ok(mut s) => {
            s.updated_at = unix_now();
            serde_json::to_vec_pretty(&*s)
        }
        Err(_) => return,
    };
    let Ok(json) = json else { return };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path)) {
        log::warn!("Could not write gateway stats to {}: {}", path.display(), e);
    }
}

pub(crate) async fn cmd_daemon(
    port: &str,
    stats_file: Option<std::path::PathBuf>,
    outbox_file: Option<std::path::PathBuf>,
) -> Result<()> {
    println!("🐕 RadioDoge Daemon — Gateway Mode (replaces serdog)");
    println!("Serial port: {}", port);
    println!("Press Ctrl-C to stop.\n");

    // NOTE: logging is already initialised in main() — calling
    // env_logger::Builder::init() a second time here panicked at daemon startup.

    let manager = Arc::new(SerialManager::new());
    let manager_for_ack = Arc::clone(&manager);
    let stats: SharedStats = Arc::new(std::sync::Mutex::new(GatewayStats::new(port, unix_now())));
    let stats_for_packets = Arc::clone(&stats);

    // Store-and-forward outbox (see radiodoge_core::store_forward).
    let initial = outbox_file
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| Outbox::from_json(&s))
        .unwrap_or_default();
    if !initial.is_empty() {
        log::info!(
            "Outbox: {} transaction(s) and {} reply(ies) carried over from the last run",
            initial.broadcasts.len(),
            initial.replies.len()
        );
    }
    let outbox: SharedOutbox = Arc::new(std::sync::Mutex::new(initial));
    let outbox_for_packets = Arc::clone(&outbox);

    // Transactions already handled this session, keyed by txid.
    //
    // A transaction addressed to the broadcast address is forwarded to its host
    // by *every* board that hears it, and the sender retransmits an
    // unacknowledged multipart part, so the same signed bytes reach a gateway
    // more than once as a matter of course. Re-POSTing them is harmless to the
    // network — the txid is identical — but the second attempt is rejected as a
    // duplicate, which used to look like a broadcast failure and cost the sender
    // its acknowledgement. Remembering what has been broadcast turns a repeat
    // into an immediate re-ACK instead.
    let seen_txs: SeenTxs = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let seen_for_outbox = Arc::clone(&seen_txs);

    let on_packet = Arc::new(move |pkt: IncomingPacket| {
        log::info!(
            "PACKET  {} → {}  cmd=0x{:02X}  rssi={}{}{}  payload={}{}",
            pkt.source.to_display_string(),
            pkt.destination.to_display_string(),
            pkt.command,
            pkt.rssi,
            pkt.snr.map(|s| format!("  snr={}", s)).unwrap_or_default(),
            if pkt.hops > 0 {
                format!("  hops={}", pkt.hops)
            } else {
                String::new()
            },
            pkt.payload_hex,
            pkt.decoded
                .as_deref()
                .map(|d| format!("  decoded={}", d))
                .unwrap_or_default(),
        );
        if let Ok(mut s) = stats_for_packets.lock() {
            s.record_packet(
                &pkt.source.to_display_string(),
                pkt.rssi,
                pkt.hops,
                unix_now(),
            );
        }

        // Store-and-forward: the node is in range right now, so release any
        // replies that could not be sent to it earlier.
        let held = outbox_for_packets
            .lock()
            .map(|mut o| o.take_replies_for(&pkt.source.to_display_string()))
            .unwrap_or_default();
        if !held.is_empty() {
            let mgr = Arc::clone(&manager_for_ack);
            let source = pkt.source.clone();
            let st = Arc::clone(&stats_for_packets);
            let ob = Arc::clone(&outbox_for_packets);
            tokio::spawn(async move {
                for r in held {
                    send_reply(
                        &r.text,
                        &mgr,
                        &source,
                        &st,
                        &ob,
                        GatewayEventKind::ReplyForwarded,
                    )
                    .await;
                }
            });
        }

        // When a signed Dogecoin transaction arrives, broadcast it to the network.
        if pkt.command == radio::CMD_DOGE_TX {
            let payload_bytes = hex::decode(&pkt.payload_hex).unwrap_or_default();
            if wallet::is_signed_tx_payload(&payload_bytes) {
                let raw_hex = pkt.payload_hex.clone();
                let txid = wallet::compute_txid(&payload_bytes);
                let mgr = Arc::clone(&manager_for_ack);
                let seen = Arc::clone(&seen_txs);
                let source = pkt.source.clone();
                let st = Arc::clone(&stats_for_packets);
                let ob = Arc::clone(&outbox_for_packets);
                stats_record(&st, GatewayEventKind::TxReceived, &source, txid.clone());
                log::info!(
                    "GATEWAY  signed tx detected ({} bytes, txid {}) from {} — broadcasting to Dogecoin network",
                    payload_bytes.len(),
                    txid,
                    source.to_display_string()
                );
                tokio::spawn(async move {
                    daemon_broadcast_and_ack(raw_hex, txid, mgr, source, seen, st, ob).await;
                });
            }
        }

        // When a balance request arrives, query the chain backends and send the result back.
        if pkt.command == radio::CMD_REQUEST_BALANCE {
            let payload_bytes = hex::decode(&pkt.payload_hex).unwrap_or_default();
            let addr = String::from_utf8_lossy(&payload_bytes)
                .trim_matches('\0')
                .trim()
                .to_string();
            if !addr.is_empty() {
                let mgr = Arc::clone(&manager_for_ack);
                let source = pkt.source.clone();
                let st = Arc::clone(&stats_for_packets);
                let ob = Arc::clone(&outbox_for_packets);
                stats_record(&st, GatewayEventKind::BalanceRequest, &source, "");
                log::info!(
                    "GATEWAY  balance request from {} for {}",
                    source.to_display_string(),
                    addr
                );
                tokio::spawn(async move {
                    daemon_fetch_and_send_balance(addr, mgr, source, st, ob).await;
                });
            }
        }
    });

    log::info!("Connecting to {} ...", port);
    manager
        .connect(port, on_packet)
        .await
        .with_context(|| format!("Failed to open serial port {}", port))?;

    let addr = manager.refresh_node_address(1500).await;
    log::info!("Connected — node address: {}", addr.to_display_string());
    if let Ok(mut s) = stats.lock() {
        s.gateway_address = Some(addr.to_display_string());
    }

    if let Some(fw) = manager.ensure_firmware_version().await {
        log::info!("Board firmware: {}", fw);
        if let Ok(mut s) = stats.lock() {
            s.firmware = Some(fw.clone());
        }
        if !radio::firmware_supports_multipart(Some(&fw)) {
            log::warn!(
                "This board predates firmware v0.4.2 (FW{}). It cannot relay a transaction \
                 larger than one {}-byte packet, so senders will be limited to the smallest \
                 possible transactions. Flash heltec-firmware-v3/ to lift the limit.",
                radio::MIN_MULTIPART_FIRMWARE,
                radio::MAX_SINGLE_PAYLOAD_LEN
            );
        }
    }

    // Put the board into gateway mode. The board only relays a host MESSAGE over
    // LoRa when this is set, and TX_ACK is a host MESSAGE — so without it a
    // gateway broadcasts transactions perfectly and the sender never hears back.
    // Asking for it here means running the daemon is enough; there is no
    // separate step in the GUI to remember.
    if let Err(e) = manager
        .send_raw(radio::build_set_gateway(&addr, true))
        .await
    {
        log::warn!("Could not enable gateway mode on the board: {}", e);
    } else {
        log::info!("Gateway mode enabled on the board (needed to radio TX_ACK back to senders)");
    }

    log::info!("Gateway ready — monitoring for signed Dogecoin transactions");

    if let Some(path) = stats_file.clone() {
        log::info!("Writing gateway stats to {}", path.display());
        let st = Arc::clone(&stats);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            loop {
                tick.tick().await;
                write_stats_file(&path, &st);
            }
        });
    }

    // Store-and-forward worker: retry queued broadcasts, expire stale work,
    // keep the dashboard gauges and the outbox file current.
    {
        let ob = Arc::clone(&outbox);
        let st = Arc::clone(&stats);
        let mgr = Arc::clone(&manager);
        let seen = seen_for_outbox;
        let path = outbox_file.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            let mut last_saved = String::new();
            loop {
                tick.tick().await;
                outbox_tick(&ob, &st, &mgr, &seen).await;
                if let Some(p) = path.as_deref() {
                    let json = ob.lock().map(|o| o.to_json()).unwrap_or_default();
                    if json != last_saved {
                        write_atomic(p, &json);
                        last_saved = json;
                    }
                }
            }
        });
    }

    // Run until Ctrl-C
    tokio::signal::ctrl_c()
        .await
        .context("Failed to listen for Ctrl-C")?;
    if let Some(p) = outbox_file.as_deref() {
        if let Ok(o) = outbox.lock() {
            write_atomic(p, &o.to_json());
        }
    }
    if let Some(path) = stats_file.as_deref() {
        write_stats_file(path, &stats);
    }

    log::info!("Shutdown signal received — disconnecting");
    manager.disconnect().await.ok();

    Ok(())
}

/// `true` if a broadcast error means the network already has this transaction.
///
/// Every node worth talking to rejects a duplicate, and each phrases it
/// differently. A duplicate is a *success* from the sender's point of view — the
/// transaction is on the network — so it must produce an acknowledgement rather
/// than a retry loop that ends in "broadcast failed".
pub(crate) fn is_duplicate_broadcast_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("already in block chain")
        || e.contains("already in the mempool")
        || e.contains("already in mempool")
        || e.contains("already known")
        || e.contains("txn-already-known")
        || e.contains("txn-already-in-mempool")
        || e.contains("transaction already exists")
        || e.contains("already exists")
        || e.contains("duplicate transaction")
}

/// Broadcast a signed transaction to the Dogecoin network with exponential-backoff
/// retry (up to 3 attempts), then radio an ACK back to the originating node.
///
/// `txid` is computed locally from the raw bytes, so the acknowledgement carries
/// the right id even when the network answers "already known" instead of
/// returning one.
pub(crate) async fn daemon_broadcast_and_ack(
    raw_hex: String,
    txid: String,
    mgr: Arc<SerialManager>,
    source: NodeAddress,
    seen: SeenTxs,
    stats: SharedStats,
    outbox: SharedOutbox,
) {
    // A transaction this gateway already put on the network only needs its
    // acknowledgement re-sent — the sender may simply not have heard the first.
    if let Some(known) = seen.lock().await.get(&txid).cloned() {
        log::info!(
            "GATEWAY  txid={} already broadcast this session — re-sending ACK",
            known
        );
        if let Ok(mut s) = stats.lock() {
            s.record_duplicate(unix_now());
        }
        daemon_send_tx_ack(&known, &mgr, &source, &stats, &outbox).await;
        return;
    }

    let mut delay_secs = 2u64;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            delay_secs *= 2;
        }
        match wallet::broadcast_raw_tx(&raw_hex).await {
            Ok(network_txid) => {
                if network_txid != txid {
                    // Not fatal — the network's answer wins — but it means the
                    // bytes were interpreted differently than expected, which is
                    // worth seeing in a gateway log.
                    log::warn!(
                        "GATEWAY  network returned txid={} but the payload hashes to {}",
                        network_txid,
                        txid
                    );
                }
                log::info!("GATEWAY  broadcast OK  txid={}", network_txid);
                seen.lock().await.insert(txid.clone(), network_txid.clone());
                stats_record(
                    &stats,
                    GatewayEventKind::TxBroadcast,
                    &source,
                    network_txid.clone(),
                );
                daemon_send_tx_ack(&network_txid, &mgr, &source, &stats, &outbox).await;
                return;
            }
            Err(e) => {
                let msg = e.to_string();
                if is_duplicate_broadcast_error(&msg) {
                    log::info!(
                        "GATEWAY  network already has txid={} — acknowledging as broadcast",
                        txid
                    );
                    seen.lock().await.insert(txid.clone(), txid.clone());
                    stats_record(
                        &stats,
                        GatewayEventKind::TxBroadcast,
                        &source,
                        format!("{} (already on network)", txid),
                    );
                    daemon_send_tx_ack(&txid, &mgr, &source, &stats, &outbox).await;
                    return;
                }
                if tx_reply::is_permanent_rejection(&msg) {
                    // The network judged the transaction itself; retrying the
                    // same bytes cannot help, so tell the sender instead of
                    // queueing it forever.
                    log::warn!(
                        "GATEWAY  network rejected txid={}: {} — sending TX_NACK",
                        txid,
                        msg
                    );
                    stats_record(&stats, GatewayEventKind::TxFailed, &source, msg.clone());
                    let nack = TxReply::Nack {
                        txid: txid.clone(),
                        reason: msg,
                    }
                    .to_text();
                    send_reply(
                        &nack,
                        &mgr,
                        &source,
                        &stats,
                        &outbox,
                        GatewayEventKind::AckSent,
                    )
                    .await;
                    return;
                }
                log::warn!(
                    "GATEWAY  broadcast attempt {}/3 failed: {}",
                    attempt + 1,
                    msg
                );
            }
        }
    }
    // Store-and-forward: keep it and keep trying. The usual cause is a gateway
    // that has lost its internet link, and the sender has no other way to reach
    // the network.
    log::warn!(
        "GATEWAY  broadcast failed after 3 attempts for txid={} — queued for retry",
        txid
    );
    let queued = outbox
        .lock()
        .map(|mut o| o.queue_broadcast(&txid, &raw_hex, &source.to_display_string(), unix_now()))
        .unwrap_or(false);
    if queued {
        stats_record(&stats, GatewayEventKind::TxQueued, &source, txid.clone());
        // Let the sender know it is held, so it does not resend in a panic.
        let q = TxReply::Queued { txid }.to_text();
        send_reply(
            &q,
            &mgr,
            &source,
            &stats,
            &outbox,
            GatewayEventKind::AckSent,
        )
        .await;
    }
}

/// One pass of the store-and-forward worker.
pub(crate) async fn outbox_tick(
    outbox: &SharedOutbox,
    stats: &SharedStats,
    mgr: &Arc<SerialManager>,
    seen: &SeenTxs,
) {
    let now = unix_now();
    let (due, expired) = match outbox.lock() {
        Ok(mut o) => {
            let before: Vec<PendingBroadcast> = o.broadcasts.clone();
            let gone = o.expire(now);
            let expired: Vec<PendingBroadcast> = before
                .into_iter()
                .filter(|b| !o.is_queued(&b.txid))
                .collect();
            if gone.1 > 0 {
                log::warn!(
                    "GATEWAY  {} held reply(ies) expired before their node was heard again",
                    gone.1
                );
            }
            (o.due_broadcasts(now), expired)
        }
        Err(_) => return,
    };
    for b in expired {
        let node = parse_node(&b.source).unwrap_or_default();
        log::error!(
            "GATEWAY  txid={} expired in the outbox after {} retries",
            b.txid,
            b.attempts
        );
        stats_record(stats, GatewayEventKind::TxFailed, &node, b.txid.clone());
        stats_record(stats, GatewayEventKind::Expired, &node, b.txid);
    }
    for b in due {
        let node = parse_node(&b.source).unwrap_or_default();
        let accepted = match wallet::broadcast_raw_tx(&b.raw_hex).await {
            Ok(id) => Some(id),
            Err(e) if is_duplicate_broadcast_error(&e.to_string()) => Some(b.txid.clone()),
            Err(e) => {
                log::info!(
                    "GATEWAY  outbox retry {} for txid={} failed: {}",
                    b.attempts + 1,
                    b.txid,
                    e
                );
                None
            }
        };
        match accepted {
            Some(id) => {
                log::info!(
                    "GATEWAY  outbox: txid={} broadcast on retry {}",
                    id,
                    b.attempts + 1
                );
                if let Ok(mut o) = outbox.lock() {
                    o.broadcast_done(&b.txid);
                }
                seen.lock().await.insert(b.txid.clone(), id.clone());
                stats_record(stats, GatewayEventKind::TxBroadcast, &node, id.clone());
                daemon_send_tx_ack(&id, mgr, &node, stats, outbox).await;
            }
            None => {
                if let Ok(mut o) = outbox.lock() {
                    o.broadcast_failed(&b.txid, unix_now());
                }
            }
        }
    }
    if let (Ok(o), Ok(mut s)) = (outbox.lock(), stats.lock()) {
        s.outbox_broadcasts = o.broadcasts.len() as u64;
        s.outbox_replies = o.replies.len() as u64;
    }
}

/// Radio `text` to `dest`; if the board will not take it, hold it until `dest` is heard again.
pub(crate) async fn send_reply(
    text: &str,
    mgr: &Arc<SerialManager>,
    dest: &NodeAddress,
    stats: &SharedStats,
    outbox: &SharedOutbox,
    ok_kind: GatewayEventKind,
) -> bool {
    let gateway_addr = mgr.get_node_address().await;
    let pkt = radio::build_message(&gateway_addr, dest, text);
    match mgr.send_raw(pkt).await {
        Ok(()) => {
            stats_record(stats, ok_kind, dest, reply_detail(text));
            true
        }
        Err(e) => {
            log::warn!(
                "GATEWAY  reply to {} not sent ({}) — held until the node is heard again",
                dest.to_display_string(),
                e
            );
            if let Ok(mut o) = outbox.lock() {
                o.queue_reply(&dest.to_display_string(), text, unix_now());
            }
            stats_record(stats, GatewayEventKind::ReplyQueued, dest, e.to_string());
            false
        }
    }
}

/// What the dashboard shows for a reply: the txid for an ACK, the amount for a balance.
pub(crate) fn reply_detail(text: &str) -> String {
    if let Some(r) = TxReply::parse(text) {
        match r {
            TxReply::Ack { txid } => txid,
            TxReply::Queued { txid } => format!("{} (queued)", txid),
            TxReply::Nack { txid, reason } => format!("{} (NACK: {})", txid, reason),
        }
    } else if let Some(k) = text
        .strip_prefix("BAL:")
        .and_then(|k| k.parse::<u64>().ok())
    {
        format!("{:.8} DOGE", k as f64 / 1e8)
    } else {
        String::new()
    }
}

/// Radio a `TX_ACK:<txid>` back to the node that sent the transaction.
///
/// The full 64-hex-character txid is included — `"TX_ACK:"` + txid is 71 bytes,
/// well inside a single packet — so the sender can actually look it up and
/// verify inclusion.
pub(crate) async fn daemon_send_tx_ack(
    txid: &str,
    mgr: &Arc<SerialManager>,
    source: &NodeAddress,
    stats: &SharedStats,
    outbox: &SharedOutbox,
) {
    if !send_reply(
        &format!("TX_ACK:{}", txid),
        mgr,
        source,
        stats,
        outbox,
        GatewayEventKind::AckSent,
    )
    .await
    {
        stats_record(stats, GatewayEventKind::AckFailed, source, "held for retry");
    }
}

/// Fetch the balance for `address` from the chain backends and send it back to `source` via radio.
/// The response is a CMD_MESSAGE with text `"BAL:{koinus}"` so the GUI can parse it.
pub(crate) async fn daemon_fetch_and_send_balance(
    address: String,
    mgr: Arc<SerialManager>,
    source: NodeAddress,
    stats: SharedStats,
    outbox: SharedOutbox,
) {
    match wallet::fetch_balance(&address).await {
        Ok(koinus) => {
            log::info!(
                "GATEWAY  balance for {}: {} koinus ({:.8} DOGE)",
                address,
                koinus,
                koinus as f64 / 1e8
            );
            // The dashboard shows the amount, never the address asked about.
            if !send_reply(
                &format!("BAL:{}", koinus),
                &mgr,
                &source,
                &stats,
                &outbox,
                GatewayEventKind::BalanceReplied,
            )
            .await
            {
                stats_record(
                    &stats,
                    GatewayEventKind::BalanceFailed,
                    &source,
                    "reply held for retry",
                );
            }
        }
        Err(e) => {
            log::warn!("GATEWAY  balance fetch failed for {}: {}", address, e);
            stats_record(
                &stats,
                GatewayEventKind::BalanceFailed,
                &source,
                e.to_string(),
            );
        }
    }
}
