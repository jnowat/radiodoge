//! Gateway mode: spawning and stopping the CLI daemon, and its stats/fleet views.

use super::*;

/// v0.3.6 — Spawn `radiodoge-cli daemon -p <port>` as a background gateway process.
/// Emits "gateway-status" event on start.
///
/// **The daemon takes ownership of the serial port.** A serial port has exactly
/// one owner: on Windows the daemon's open would simply fail, and on Linux both
/// processes read the same device and each receives a random subset of the
/// bytes — on the path that carries transactions. Since the button is normally
/// pressed for the port the app itself is holding, the app releases it here and
/// takes it back in [`stop_gateway`], rather than leaving two readers fighting
/// over one board.
#[tauri::command]
pub(crate) async fn start_gateway(
    port: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    // Stop any existing daemon first
    {
        let mut gp = state.gateway_process.lock().await;
        if let Some(ref mut child) = *gp {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *gp = None;
        *state.gateway_owned_port.lock().await = None;
    }

    // Hand the port over if we are the one holding it. The auto-reconnect
    // watchdog has to be stopped first, or it will reopen the port underneath
    // the daemon a couple of seconds later.
    let holding_this_port = state
        .current_port
        .lock()
        .await
        .as_deref()
        .is_some_and(|p| p == port);
    if holding_this_port {
        log::info!("Releasing {} so the gateway daemon can own it", port);
        state.reconnect_enabled.store(false, Ordering::Relaxed);
        state.connection_generation.fetch_add(1, Ordering::SeqCst);
        state
            .serial
            .disconnect()
            .await
            .map_err(radiodoge_core::errors::friendly)?;
        *state.current_port.lock().await = None;
        let _ = app.emit("connection-status", ConnectionStatusEvent::disconnected());
        #[cfg(desktop)]
        tray::update_tray_status(&app, false, None);
    }

    // v0.3.7 — Find radiodoge-cli: check same dir as this executable first (bundled),
    // then fall back to PATH.  This eliminates "program not found" for packaged apps.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let cli_name = if cfg!(windows) {
        "radiodoge-cli.exe"
    } else {
        "radiodoge-cli"
    };
    let cli_path = exe_dir
        .map(|d| d.join(cli_name))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| cli_name.to_string()); // fall back to PATH

    // The daemon writes its counters here for the gateway dashboard. Remove the
    // previous run's snapshot so the dashboard never shows stale numbers.
    let stats_path = gateway_stats_path(&app);
    if let Some(ref p) = stats_path {
        if let Some(dir) = p.parent() {
            let _ = tokio::fs::create_dir_all(dir).await;
        }
        let _ = tokio::fs::remove_file(p).await;
    }
    let mut daemon_args: Vec<String> = vec!["daemon".into(), "-p".into(), port.clone()];
    if let Some(ref p) = stats_path {
        daemon_args.push("--stats-file".into());
        daemon_args.push(p.to_string_lossy().into_owned());
    }
    // Store-and-forward: transactions still waiting for the network survive a
    // gateway restart. Unlike the stats snapshot, this file is kept between runs.
    if let Some(outbox) = stats_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join("gateway-outbox.json"))
    {
        daemon_args.push("--outbox-file".into());
        daemon_args.push(outbox.to_string_lossy().into_owned());
    }

    let mut cmd = tokio::process::Command::new(&cli_path);
    cmd.args(&daemon_args).stdin(std::process::Stdio::null());
    // The release GUI is a windowed (non-console) app, so a console child would
    // pop up its own black console window — and closing that window kills the
    // gateway. Run it windowless and keep its output in a log file instead.
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    if let Some((out, err)) = daemon_log_path(&app).and_then(|p| {
        let f = std::fs::File::create(&p).ok()?;
        Some((f.try_clone().ok()?, f))
    }) {
        cmd.stdout(out).stderr(err);
    }
    let child = cmd
        // Without this the daemon outlives the app: closing the window would
        // leave an orphan holding the serial port, and the next launch could not
        // open the board at all.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to spawn '{}': {}. \
             Bundle radiodoge-cli next to the app executable, or add it to PATH.",
                cli_path, e
            )
        })?;

    // Spawning succeeding only means the binary launched. The daemon can still
    // exit immediately — no board on that port, no permission, port still held —
    // and reporting "Gateway Online" for a process that is already dead is worse
    // than reporting nothing. Give it a moment and check it is still alive.
    let mut child = child;
    tokio::time::sleep(Duration::from_millis(700)).await;
    match child.try_wait() {
        Ok(Some(status)) => {
            let mut msg = format!(
                "The gateway daemon exited immediately ({}). The most common cause is that \
                 {} could not be opened — check the board is attached and that nothing else \
                 is using the port.",
                status, port
            );
            if let Some(tail) = daemon_log_path(&app)
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|t| log_tail(&t, 3))
            {
                if !tail.is_empty() {
                    msg.push_str(&format!(" Daemon said: {}", tail));
                }
            }
            // We took the port away for it; give it back rather than leaving the
            // app disconnected after a failure it did not cause.
            if holding_this_port {
                match connect_port(port.clone(), state, app).await {
                    Ok(()) => msg.push_str(" Reconnected the app to the board."),
                    Err(e) => msg.push_str(&format!(" Reconnecting the app also failed: {}", e)),
                }
            }
            return Err(msg);
        }
        Ok(None) => {} // still running, as expected
        Err(e) => log::warn!("Could not check the gateway daemon's state: {}", e),
    }

    *state.gateway_process.lock().await = Some(child);
    *state.gateway_owned_port.lock().await = Some(port.clone());
    let _ = app.emit(
        "gateway-status",
        serde_json::json!({ "online": true, "port": port, "portReleased": holding_this_port }),
    );
    log::info!("Gateway daemon started on port {}", port);
    Ok(())
}

/// Where the gateway daemon's console output goes (overwritten on each start).
pub(crate) fn daemon_log_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    gateway_stats_path(app).and_then(|p| p.parent().map(|d| d.join("gateway-daemon.log")))
}

/// Where the gateway daemon writes its dashboard snapshot.
pub(crate) fn gateway_stats_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("gateway-stats.json"))
}

/// Gateway dashboard: the running daemon's latest counters.
///
/// `running` reflects the daemon process this app started; `stats` is `None`
/// until the daemon has written its first snapshot (a few seconds after start).
/// A daemon that died on its own reports `running: false` while the app still
/// shows Stop, which reclaims the port.
#[tauri::command]
pub(crate) async fn get_gateway_stats(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let running = {
        let mut gp = state.gateway_process.lock().await;
        match gp.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            // Exited (or unknowable). Leave the handle and the owned port alone so
            // Stop still reclaims the serial port; the dashboard says it stopped.
            _ => false,
        }
    };
    let stats = match gateway_stats_path(&app) {
        Some(p) => match tokio::fs::read(&p).await {
            Ok(bytes) => {
                serde_json::from_slice::<radiodoge_core::gateway_stats::GatewayStats>(&bytes).ok()
            }
            Err(_) => None,
        },
        None => None,
    };
    Ok(serde_json::json!({ "running": running, "stats": stats }))
}

/// v0.3.21 — Multi-gateway dashboard.
///
/// Reads this app's own daemon snapshot plus every extra stats file the user
/// added (other gateways writing `--stats-file` into a shared folder), and
/// returns each source's status alongside the merged fleet summary.
#[tauri::command]
pub(crate) async fn get_fleet_stats(
    paths: Vec<String>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    use radiodoge_core::fleet::{aggregate, is_online, FleetMember};
    use radiodoge_core::gateway_stats::GatewayStats;
    pub(crate) const MAX_BYTES: u64 = 4 * 1024 * 1024;

    let mut sources: Vec<(String, Option<std::path::PathBuf>)> = Vec::new();
    if let Some(p) = gateway_stats_path(&app) {
        sources.push(("This PC".into(), Some(p)));
    }
    for raw in paths.iter().take(32) {
        let t = raw.trim();
        if !t.is_empty() {
            sources.push((String::new(), Some(std::path::PathBuf::from(t))));
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut members = Vec::new();
    let mut rows = Vec::new();
    let mut used = std::collections::HashSet::new();
    for (i, (fixed, path)) in sources.into_iter().enumerate() {
        let Some(path) = path else { continue };
        let read = async {
            let meta = tokio::fs::metadata(&path)
                .await
                .map_err(radiodoge_core::errors::friendly)?;
            if meta.len() > MAX_BYTES {
                return Err("file is larger than 4 MB".to_string());
            }
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(radiodoge_core::errors::friendly)?;
            serde_json::from_slice::<GatewayStats>(&bytes)
                .map_err(|e| format!("not a stats file: {e}"))
        }
        .await;
        let is_local = i == 0 && fixed == "This PC";
        let base = if !fixed.is_empty() {
            fixed
        } else if let Ok(s) = &read {
            s.gateway_address.clone().unwrap_or_else(|| {
                path.file_stem()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "gateway".into())
            })
        } else {
            path.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "gateway".into())
        };
        let mut label = base.clone();
        let mut n = 2;
        while !used.insert(label.clone()) {
            label = format!("{base} #{n}");
            n += 1;
        }
        match read {
            Ok(stats) => {
                rows.push(serde_json::json!({
                    "label": label, "path": path.to_string_lossy(), "local": is_local,
                    "ok": true, "online": is_online(&stats, now), "updatedAt": stats.updated_at,
                    "port": stats.port, "address": stats.gateway_address,
                    "packetsHeard": stats.packets_heard, "txBroadcast": stats.tx_broadcast,
                    "txFailed": stats.tx_failed,
                    "outbox": stats.outbox_broadcasts + stats.outbox_replies,
                }));
                members.push(FleetMember { label, stats });
            }
            // A missing local snapshot just means this PC is not running a gateway.
            Err(_) if is_local => {}
            Err(e) => rows.push(serde_json::json!({
                "label": label, "path": path.to_string_lossy(), "local": false, "ok": false, "error": e,
            })),
        }
    }
    let summary = aggregate(&members, now);
    Ok(serde_json::json!({ "now": now, "gateways": rows, "summary": summary }))
}

/// v0.3.6 — Stop the background gateway daemon.
///
/// Reconnects to the port the daemon was given, if the app was the one that
/// released it — otherwise stopping the gateway would leave the user staring at
/// a disconnected app with no indication that they need to press Connect again.
#[tauri::command]
pub(crate) async fn stop_gateway(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    if let Some(mut child) = state.gateway_process.lock().await.take() {
        let _ = child.kill().await;
        // The OS releases the port when the process actually exits, which is not
        // instantaneous; reopening too early fails with "access denied".
        let _ = child.wait().await;
    }
    let released = state.gateway_owned_port.lock().await.take();
    let _ = app.emit("gateway-status", serde_json::json!({ "online": false }));
    log::info!("Gateway daemon stopped");

    if let Some(port) = released {
        if !state.serial.is_connected() {
            log::info!("Reclaiming {} now the gateway daemon has exited", port);
            if let Err(e) = connect_port(port.clone(), state, app).await {
                // Not fatal: the user can press Connect. Say so rather than
                // reporting a failure to stop the gateway, which did stop.
                log::warn!(
                    "Could not reopen {} after stopping the gateway: {}",
                    port,
                    e
                );
            }
        }
    }
    Ok(())
}

/// The last `n` non-empty lines of a log, joined with " | ".
fn log_tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

#[cfg(test)]
mod log_tail_tests {
    #[test]
    fn keeps_last_non_empty_lines() {
        assert_eq!(super::log_tail("a\n\nb\r\nc\n  \n", 2), "b | c");
        assert_eq!(super::log_tail("", 3), "");
        assert_eq!(super::log_tail("only", 3), "only");
    }
}
