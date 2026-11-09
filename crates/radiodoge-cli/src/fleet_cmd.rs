//! Fleet dashboard HTTP: serve aggregated gateway `--stats-file` snapshots.
//!
//! ```text
//! radiodoge-cli fleet serve --dir /var/radiodoge/stats --bind 127.0.0.1:8787
//! ```
//!
//! Endpoints (JSON):
//! - `GET /health` — liveness
//! - `GET /` and `GET /summary` — `{ now, gateways, summary }` (same shape as the GUI)
//!
//! One-shot (no server):
//! ```text
//! radiodoge-cli fleet summary --dir /var/radiodoge/stats
//! ```

use anyhow::{bail, Context, Result};
use radiodoge_core::fleet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// Run until Ctrl-C. Re-reads the stats directory on every request.
pub async fn cmd_fleet_serve(dir: PathBuf, bind: String) -> Result<()> {
    if !dir.is_dir() {
        bail!("stats directory does not exist: {}", dir.display());
    }
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    println!("🐕 Fleet HTTP on http://{bind}  (dir {})", dir.display());
    println!("   GET /health   GET /summary");

    let dir = Arc::new(dir);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                println!("Fleet HTTP stopping.");
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _peer)) => {
                        let dir = Arc::clone(&dir);
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, &dir).await {
                                log::debug!("fleet http client: {e:#}");
                            }
                        });
                    }
                    Err(e) => log::warn!("fleet http accept: {e}"),
                }
            }
        }
    }
    Ok(())
}

async fn handle_client(mut stream: TcpStream, dir: &std::path::Path) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .and_then(|line| {
            let mut parts = line.split_whitespace();
            let method = parts.next()?;
            let path = parts.next()?;
            (method == "GET").then_some(path)
        })
        .unwrap_or("/");

    let (status, body, ctype) = match path.split('?').next().unwrap_or(path) {
        "/health" => ("200 OK", br#"{"ok":true}"#.to_vec(), "application/json"),
        "/" | "/summary" => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            match fleet::response_from_dir(dir, now) {
                Ok(resp) => (
                    "200 OK",
                    serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec()),
                    "application/json",
                ),
                Err(e) => {
                    let msg = serde_json::json!({ "ok": false, "error": e });
                    (
                        "500 Internal Server Error",
                        serde_json::to_vec(&msg).unwrap_or_else(|_| b"{}".to_vec()),
                        "application/json",
                    )
                }
            }
        }
        _ => (
            "404 Not Found",
            br#"{"ok":false,"error":"not found"}"#.to_vec(),
            "application/json",
        ),
    };

    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}

/// Print one fleet JSON snapshot to stdout and exit.
pub fn cmd_fleet_summary(dir: PathBuf) -> Result<()> {
    if !dir.is_dir() {
        bail!("stats directory does not exist: {}", dir.display());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let resp = fleet::response_from_dir(&dir, now).map_err(|e| anyhow::anyhow!(e))?;
    println!("{}", serde_json::to_string_pretty(&resp)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiodoge_core::fleet::response_from_dir;
    use radiodoge_core::gateway_stats::GatewayStats;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample(addr: &str, updated: u64, packets: u64) -> GatewayStats {
        GatewayStats {
            updated_at: updated,
            gateway_address: Some(addr.into()),
            port: "COM3".into(),
            packets_heard: packets,
            tx_broadcast: packets,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn serve_summary_over_http() {
        let dir = std::env::temp_dir().join(format!(
            "rd-fleet-http-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let now = 1_700_000_100u64;
        std::fs::write(
            dir.join("a.json"),
            serde_json::to_vec(&sample("10.0.1", now, 4)).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("b.json"),
            serde_json::to_vec(&sample("10.0.2", now, 6)).unwrap(),
        )
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dir_clone = dir.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_client(stream, &dir_clone).await.unwrap();
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET /summary HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8_lossy(&resp);
        assert!(text.contains("200 OK"), "{text}");
        assert!(text.contains("\"gateways\""), "{text}");
        assert!(
            text.contains("10.0.1")
                || text.contains("packetsHeard")
                || text.contains("packets_heard")
                || text.contains("\"packetsHeard\"")
                || text.contains("summary"),
            "{text}"
        );
        // body must include aggregated packet count 10
        assert!(
            text.contains("10")
                || text.contains("\"packetsHeard\":10")
                || text.contains("packetsHeard\":10")
                || text.contains("packets_heard\":10"),
            "{text}"
        );
        server.await.unwrap();
        let r = response_from_dir(&dir, now).unwrap();
        assert_eq!(r.summary.packets_heard, 10);
        assert_eq!(r.summary.gateways, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
