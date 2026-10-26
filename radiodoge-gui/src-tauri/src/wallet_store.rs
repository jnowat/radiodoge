//! Encrypted wallet persistence and the address book.

use super::*;

// ─── v0.3.8: Persistent Wallet ───────────────────────────────────────────────

/// v0.3.16 — Encrypt and persist the wallet to app-local storage.
/// The WIF private key is encrypted with ChaCha20-Poly1305 (argon2id KDF, 64 MiB).
/// Marker the frontend matches on to offer "replace it anyway".
///
/// A distinguishable prefix rather than prose, so the UI can react to this one
/// case without pattern-matching an English sentence.
pub const ERR_DIFFERENT_WALLET_SAVED: &str = "different_wallet_saved:";

/// The address of the wallet currently saved on disk, if there is one.
///
/// Read from the plaintext `address` field of the encrypted file, so it needs no
/// passphrase — the point is to know *which* wallet is stored without being able
/// to open it.
pub(crate) async fn saved_wallet_address(app: &AppHandle) -> Option<String> {
    let path = app.path().app_data_dir().ok()?.join("wallet.json");
    let json = tokio::fs::read_to_string(&path).await.ok()?;
    if let Ok(enc) = serde_json::from_str::<wallet::EncryptedWalletFile>(&json) {
        return Some(enc.address);
    }
    // Legacy plaintext format (pre-v0.3.16).
    serde_json::from_str::<WalletInfo>(&json).ok().map(|w| w.address)
}

#[tauri::command]
pub(crate) async fn save_wallet(
    wallet_info: WalletInfo,
    passphrase: String,
    allow_replace: Option<bool>,
    app: AppHandle,
) -> Result<(), String> {
    if passphrase.len() < 8 {
        return Err("Passphrase must be at least 8 characters.".to_string());
    }

    // Never silently replace a *different* wallet. There is one wallet file, and
    // overwriting it destroys the only copy of the previous key — which is
    // trivially reachable: dismiss the unlock prompt on launch, generate a new
    // wallet, save it. The UI cannot be the only thing standing between a user
    // and that, so the refusal lives here and has to be overridden explicitly.
    if !allow_replace.unwrap_or(false) {
        if let Some(existing) = saved_wallet_address(&app).await {
            if existing != wallet_info.address {
                return Err(format!("{}{}", ERR_DIFFERENT_WALLET_SAVED, existing));
            }
        }
    }
    let encrypted = wallet::encrypt_wallet(&wallet_info, &passphrase)
        .map_err(radiodoge_core::errors::friendly)?;

    // Prove the ciphertext decrypts back to this exact wallet before anything is
    // written. This file is often the user's only copy of the key, and a wallet
    // that saved "successfully" but cannot be reopened is indistinguishable from
    // losing the coins. The cost is one extra argon2 pass on save.
    match wallet::decrypt_wallet(&encrypted, &passphrase) {
        Ok(check) if check.private_key_wif == wallet_info.private_key_wif
            && check.address == wallet_info.address => {}
        Ok(_) => {
            return Err("Refusing to save: the encrypted wallet did not decrypt back to the \
                        same key. Nothing was written; your existing wallet file is untouched."
                .to_string())
        }
        Err(e) => {
            return Err(format!(
                "Refusing to save: the encrypted wallet could not be decrypted back ({}). \
                 Nothing was written; your existing wallet file is untouched.",
                e
            ))
        }
    }

    let dir = app.path().app_data_dir().map_err(radiodoge_core::errors::friendly)?;
    tokio::fs::create_dir_all(&dir).await.map_err(radiodoge_core::errors::friendly)?;
    let path = dir.join("wallet.json");
    let json = serde_json::to_string_pretty(&encrypted).map_err(radiodoge_core::errors::friendly)?;
    write_file_atomically(&path, &json).await?;
    log::info!("Encrypted wallet saved to disk.");
    Ok(())
}

/// Returns true if a wallet.json exists on disk (so the frontend can show a passphrase prompt).
#[tauri::command]
pub(crate) async fn wallet_needs_passphrase(app: AppHandle) -> bool {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("wallet.json"),
        Err(_) => return false,
    };
    tokio::fs::metadata(&path).await.is_ok()
}

/// v0.3.16 — Load and decrypt a wallet from disk.
/// - Encrypted format (v0.3.16+): requires a non-empty passphrase.
/// - Legacy plaintext format (pre-v0.3.16): loaded directly; the caller should prompt
///   the user to re-save with a passphrase.
/// Returns `Err("passphrase_required")` if the file is encrypted but no passphrase was supplied.
#[tauri::command]
pub(crate) async fn load_saved_wallet(passphrase: Option<String>, app: AppHandle) -> Result<Option<WalletInfo>, String> {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("wallet.json"),
        Err(_) => return Ok(None),
    };
    let json = match tokio::fs::read_to_string(&path).await {
        Ok(j) => j,
        Err(_) => return Ok(None),
    };
    // Try encrypted format first (v0.3.16+).
    if let Ok(enc) = serde_json::from_str::<wallet::EncryptedWalletFile>(&json) {
        let pp = match &passphrase {
            Some(p) if !p.is_empty() => p.as_str(),
            _ => return Err("passphrase_required".to_string()),
        };
        let info = wallet::decrypt_wallet(&enc, pp).map_err(radiodoge_core::errors::friendly)?;
        return Ok(Some(info));
    }
    // Legacy plaintext format (pre-v0.3.16) — load directly.
    if let Ok(w) = serde_json::from_str::<WalletInfo>(&json) {
        log::warn!("Loaded legacy plaintext wallet — user should re-save with encryption.");
        return Ok(Some(w));
    }
    Err("wallet file is corrupted or in an unrecognised format".to_string())
}

/// v0.3.8 — Delete the saved wallet from disk (user explicitly cleared it).
#[tauri::command]
pub(crate) async fn delete_saved_wallet(app: AppHandle) -> Result<(), String> {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("wallet.json"),
        Err(_) => return Ok(()),
    };
    let _ = tokio::fs::remove_file(&path).await;
    Ok(())
}

// ─── v0.3.8: Address Book ────────────────────────────────────────────────────

/// v0.3.8 — Load the address book from app-local storage.
/// Returns an empty array if no address book has been saved.
#[tauri::command]
pub(crate) async fn load_address_book(app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("address_book.json"),
        Err(_) => return Ok(vec![]),
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(json) => match serde_json::from_str::<Vec<serde_json::Value>>(&json) {
            Ok(entries) => Ok(entries),
            Err(e) => {
                eprintln!("[addrbook] address_book.json parse failed ({}), starting fresh", e);
                Ok(vec![])
            }
        },
        Err(_) => Ok(vec![]),
    }
}

/// v0.3.8 — Save the address book to app-local storage.
#[tauri::command]
pub(crate) async fn save_address_book(entries: Vec<serde_json::Value>, app: AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(radiodoge_core::errors::friendly)?;
    tokio::fs::create_dir_all(&dir).await.map_err(radiodoge_core::errors::friendly)?;
    let path = dir.join("address_book.json");
    let json = serde_json::to_string_pretty(&entries).map_err(radiodoge_core::errors::friendly)?;
    write_file_atomically(&path, &json).await?;
    Ok(())
}
