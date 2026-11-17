//! One-time migration from Electron's Windows safeStorage to Credential Manager.
use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use std::{path::PathBuf, ptr};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB},
};

pub fn legacy_directory() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("ai-router-desktop"))
}

pub fn read_token() -> Result<Option<String>, String> {
    let Some(dir) = legacy_directory() else {
        return Ok(None);
    };
    let settings_path = dir.join("settings.json");
    if !settings_path.exists() {
        return Ok(None);
    }
    let settings: Value =
        serde_json::from_slice(&std::fs::read(settings_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let Some(encrypted) = settings["encryptedToken"].as_str() else {
        return Ok(None);
    };
    let state: Value =
        serde_json::from_slice(&std::fs::read(dir.join("Local State")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let protected = STANDARD
        .decode(
            state["os_crypt"]["encrypted_key"]
                .as_str()
                .ok_or("旧版加密密钥缺失")?,
        )
        .map_err(|e| e.to_string())?;
    if !protected.starts_with(b"DPAPI") {
        return Err("旧版加密密钥格式不受支持".into());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: (protected.len() - 5) as u32,
        pbData: protected[5..].as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            0,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("无法解密旧版密钥保护信息".into());
    }
    if output.pbData.is_null() || output.cbData == 0 {
        if !output.pbData.is_null() {
            unsafe {
                LocalFree(output.pbData as _);
            }
        }
        return Err("旧版密钥保护信息为空".into());
    }
    let key = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as _);
    }
    let data = STANDARD.decode(encrypted).map_err(|e| e.to_string())?;
    if data.len() < 31 || !(data.starts_with(b"v10") || data.starts_with(b"v11")) {
        return Err("旧版密钥格式不受支持".into());
    }
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "旧版加密密钥长度无效")?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&data[3..15]), &data[15..])
        .map_err(|_| "无法解密旧版本机 API Key")?;
    let token = String::from_utf8(plaintext).map_err(|_| "旧版本机 API Key 无效")?;
    if token.len() < 24 {
        return Err("旧版本机 API Key 长度无效".into());
    }
    Ok(Some(token))
}
