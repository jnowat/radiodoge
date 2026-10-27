//! Offline signing on the phone (and desktop): import a UTXO snapshot written
//! by `radiodoge-cli utxo-snapshot`, then sign with no internet. The snapshot
//! holds only public chain data and lives in memory; it is never written to
//! disk by the app.

use super::*;
use radiodoge_core::backend::UtxoSnapshot;

/// What the UI shows about the loaded snapshot.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInfo {
    pub address: String,
    pub fetched_at: u64,
    pub utxo_count: usize,
    pub total_doge: f64,
}

fn info(s: &UtxoSnapshot) -> SnapshotInfo {
    SnapshotInfo {
        address: s.address.clone(),
        fetched_at: s.fetched_at,
        utxo_count: s.utxos.len(),
        total_doge: s.utxos.iter().map(|u| u.value).sum::<u64>() as f64 / 1e8,
    }
}

/// Parse and sanity-check a snapshot file's text.
pub(crate) fn parse_snapshot(json: &str) -> Result<UtxoSnapshot, String> {
    let snap: UtxoSnapshot =
        serde_json::from_str(json).map_err(|e| format!("Not a UTXO snapshot file: {}", e))?;
    if !wallet::is_valid_address(&snap.address) {
        return Err(format!("Snapshot address '{}' is not a valid Dogecoin address", snap.address));
    }
    if snap.utxos.is_empty() {
        return Err("Snapshot has no UTXOs — nothing to spend".to_string());
    }
    if let Some(u) = snap.utxos.iter().find(|u| u.txid.len() != 64 || !u.txid.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(format!("Snapshot has a malformed txid: {}", u.txid));
    }
    Ok(snap)
}

/// Decide how to sign: from the snapshot when one is loaded for this wallet's
/// address (an explicit fee is then required, since no rate can be fetched),
/// otherwise over the network as before.
pub(crate) async fn sign_payload(
    snapshot: &Mutex<Option<UtxoSnapshot>>,
    wif: &str,
    to: &str,
    amount_doge: f64,
    fee_doge: Option<f64>,
) -> Result<Vec<u8>, String> {
    let snap = snapshot.lock().await.clone();
    if let Some(s) = snap {
        let from = wallet::import_wif(wif).map_err(|e| format!("Transaction signing failed: {}", e))?.address;
        if from == s.address {
            let fee = fee_doge.ok_or_else(|| {
                "Offline snapshot loaded: enter a network fee (e.g. 0.01 DOGE) — there is no internet to estimate one."
                    .to_string()
            })?;
            return wallet::build_signed_transaction_from_utxos(wif, to, amount_doge, fee, s.utxos)
                .map_err(|e| format!("Offline signing failed: {}", e));
        }
    }
    Ok(wallet::build_signed_transaction_with_fee(wif, to, amount_doge, fee_doge)
        .await
        .map_err(|e| format!("Transaction signing failed: {}", e))?
        .0)
}

#[tauri::command]
pub(crate) async fn import_utxo_snapshot(json: String, state: State<'_, AppState>) -> Result<SnapshotInfo, String> {
    let snap = parse_snapshot(&json)?;
    let i = info(&snap);
    *state.utxo_snapshot.lock().await = Some(snap);
    Ok(i)
}

#[tauri::command]
pub(crate) async fn utxo_snapshot_info(state: State<'_, AppState>) -> Result<Option<SnapshotInfo>, String> {
    Ok(state.utxo_snapshot.lock().await.as_ref().map(info))
}

#[tauri::command]
pub(crate) async fn clear_utxo_snapshot(state: State<'_, AppState>) -> Result<(), String> {
    *state.utxo_snapshot.lock().await = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiodoge_core::backend::Utxo;

    // A fresh random key each run; never funded, never stored.
    fn test_wif() -> String {
        wallet::generate_keypair().unwrap().private_key_wif
    }

    fn snap_for(addr: &str) -> UtxoSnapshot {
        UtxoSnapshot {
            address: addr.into(),
            fetched_at: 1,
            utxos: vec![Utxo { txid: "ab".repeat(32), vout: 0, value: 500_000_000, confirmations: 10, coinbase: false }],
        }
    }

    #[test]
    fn parse_rejects_bad_files() {
        let addr = wallet::import_wif(&test_wif()).unwrap().address;
        assert!(parse_snapshot("{").is_err());
        let mut s = snap_for(&addr);
        s.utxos.clear();
        assert!(parse_snapshot(&serde_json::to_string(&s).unwrap()).is_err());
        let mut s = snap_for(&addr);
        s.utxos[0].txid = "zz".into();
        assert!(parse_snapshot(&serde_json::to_string(&s).unwrap()).is_err());
        let ok = parse_snapshot(&serde_json::to_string(&snap_for(&addr)).unwrap()).unwrap();
        assert_eq!(info(&ok).utxo_count, 1);
        assert!((info(&ok).total_doge - 5.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn signs_offline_from_snapshot_and_needs_fee() {
        let wif = test_wif();
        let addr = wallet::import_wif(&wif).unwrap().address;
        let m = Mutex::new(Some(snap_for(&addr)));
        let to = wallet::import_wif(&test_wif()).unwrap().address;
        let err = sign_payload(&m, &wif, &to, 1.0, None).await.unwrap_err();
        assert!(err.contains("enter a network fee"));
        let raw = sign_payload(&m, &wif, &to, 1.0, Some(0.01)).await.unwrap();
        let (from, outs) = wallet::verify_signed_tx(&raw).unwrap();
        assert_eq!(from, addr);
        assert!(outs.contains(&(100_000_000, to)));
    }
}
