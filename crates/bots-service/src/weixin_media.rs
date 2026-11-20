//! 微信 CDN 媒体解密（AES-128-ECB + PKCS7）。
//!
//! 真源：`weixinProvider.ts` 的 `parseWeixinAesKey` / `decryptWeixinCdnMedia`。
//! 微信 iLink CDN 返回 AES-128-ECB + PKCS7 padding 的密文字节；直接保存会得到不可识别的
//! 文件，因此下载后必须先解密。
//!
//! 纯 Rust 实现（不引第三方 AES crate）：S-box 在初始化时按 GF(2^8) 乘法逆 + 仿射变换
//! 生成，逆 S-box 由置换反推；以 NIST SP800-38A 向量对拍。

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::http::HttpJsonPort;

static SBOX: OnceLock<[u8; 256]> = OnceLock::new();
static INV_SBOX: OnceLock<[u8; 256]> = OnceLock::new();

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut result = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            result ^= a;
        }
        let high = a & 0x80 != 0;
        a <<= 1;
        if high {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    result
}

fn rotl8(value: u8, shift: u32) -> u8 {
    value.rotate_left(shift)
}

fn build_sbox() -> [u8; 256] {
    let mut table = [0u8; 256];
    for x in 0..256u32 {
        let x = x as u8;
        let inverse = if x == 0 {
            0
        } else {
            // GF(2^8) 乘法逆：a^254（a^255 = 1）。
            let mut result = 1u8;
            let mut base = x;
            let mut exponent = 254u32;
            while exponent > 0 {
                if exponent & 1 == 1 {
                    result = gf_mul(result, base);
                }
                base = gf_mul(base, base);
                exponent >>= 1;
            }
            result
        };
        table[x as usize] = inverse
            ^ rotl8(inverse, 1)
            ^ rotl8(inverse, 2)
            ^ rotl8(inverse, 3)
            ^ rotl8(inverse, 4)
            ^ 0x63;
    }
    table
}

fn sbox() -> &'static [u8; 256] {
    SBOX.get_or_init(build_sbox)
}

fn inv_sbox() -> &'static [u8; 256] {
    INV_SBOX.get_or_init(|| {
        let mut inverse = [0u8; 256];
        for (index, value) in sbox().iter().enumerate() {
            inverse[*value as usize] = index as u8;
        }
        inverse
    })
}

fn expand_key(key: &[u8; 16]) -> [[u8; 16]; 11] {
    const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];
    let mut words = [[0u8; 4]; 44];
    for index in 0..4 {
        words[index] = [key[index * 4], key[index * 4 + 1], key[index * 4 + 2], key[index * 4 + 3]];
    }
    for index in 4..44 {
        let mut temp = words[index - 1];
        if index % 4 == 0 {
            temp = [
                sbox()[temp[1] as usize] ^ RCON[index / 4 - 1],
                sbox()[temp[2] as usize],
                sbox()[temp[3] as usize],
                sbox()[temp[0] as usize],
            ];
        }
        let previous = words[index - 4];
        words[index] = [
            previous[0] ^ temp[0],
            previous[1] ^ temp[1],
            previous[2] ^ temp[2],
            previous[3] ^ temp[3],
        ];
    }
    let mut round_keys = [[0u8; 16]; 11];
    for round in 0..11 {
        for column in 0..4 {
            let word = words[round * 4 + column];
            round_keys[round][column * 4] = word[0];
            round_keys[round][column * 4 + 1] = word[1];
            round_keys[round][column * 4 + 2] = word[2];
            round_keys[round][column * 4 + 3] = word[3];
        }
    }
    round_keys
}

fn add_round_key(state: &mut [u8; 16], round_key: &[u8; 16]) {
    for index in 0..16 {
        state[index] ^= round_key[index];
    }
}

fn inv_shift_rows(state: &mut [u8; 16]) {
    // 列优先布局：state[r + 4c]；第 r 行循环右移 r。
    let copy = *state;
    for row in 0..4 {
        for column in 0..4 {
            let source_column = (column + 4 - row) % 4;
            state[row + 4 * column] = copy[row + 4 * source_column];
        }
    }
}

fn inv_sub_bytes(state: &mut [u8; 16]) {
    for byte in state.iter_mut() {
        *byte = inv_sbox()[*byte as usize];
    }
}

fn inv_mix_columns(state: &mut [u8; 16]) {
    for column in 0..4 {
        let index = column * 4;
        let a = [state[index], state[index + 1], state[index + 2], state[index + 3]];
        state[index] = gf_mul(a[0], 0x0e) ^ gf_mul(a[1], 0x0b) ^ gf_mul(a[2], 0x0d) ^ gf_mul(a[3], 0x09);
        state[index + 1] = gf_mul(a[0], 0x09) ^ gf_mul(a[1], 0x0e) ^ gf_mul(a[2], 0x0b) ^ gf_mul(a[3], 0x0d);
        state[index + 2] = gf_mul(a[0], 0x0d) ^ gf_mul(a[1], 0x09) ^ gf_mul(a[2], 0x0e) ^ gf_mul(a[3], 0x0b);
        state[index + 3] = gf_mul(a[0], 0x0b) ^ gf_mul(a[1], 0x0d) ^ gf_mul(a[2], 0x09) ^ gf_mul(a[3], 0x0e);
    }
}

/// 解密一个 16 字节块（AES-128）。
pub fn decrypt_block(block: &[u8; 16], key: &[u8; 16]) -> [u8; 16] {
    let round_keys = expand_key(key);
    let mut state = *block;
    add_round_key(&mut state, &round_keys[10]);
    for round in (1..10).rev() {
        inv_shift_rows(&mut state);
        inv_sub_bytes(&mut state);
        add_round_key(&mut state, &round_keys[round]);
        inv_mix_columns(&mut state);
    }
    inv_shift_rows(&mut state);
    inv_sub_bytes(&mut state);
    add_round_key(&mut state, &round_keys[0]);
    state
}

fn parse_aes_key(value: &str) -> Option<[u8; 16]> {
    let trimmed = value.trim();
    if trimmed.len() == 32 && trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
        let mut key = [0u8; 16];
        for index in 0..16 {
            key[index] = u8::from_str_radix(&trimmed[index * 2..index * 2 + 2], 16).ok()?;
        }
        return Some(key);
    }
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, trimmed).ok()?;
    if decoded.len() == 16 {
        let key: [u8; 16] = decoded.try_into().ok()?;
        return Some(key);
    }
    if let Ok(text) = String::from_utf8(decoded) {
        let text = text.trim();
        if text.len() == 32 && text.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return parse_aes_key(text);
        }
    }
    None
}

fn pkcs7_unpad(data: Vec<u8>) -> Result<Vec<u8>, String> {
    let Some(&padding) = data.last() else {
        return Err("Weixin attachment AES padding missing.".to_string());
    };
    let padding = padding as usize;
    if padding == 0 || padding > 16 || padding > data.len() {
        return Err("Weixin attachment AES padding is invalid.".to_string());
    }
    if !data[data.len() - padding..].iter().all(|byte| *byte as usize == padding) {
        return Err("Weixin attachment AES padding is invalid.".to_string());
    }
    Ok(data[..data.len() - padding].to_vec())
}

/// `decryptWeixinCdnMedia`：AES-128-ECB 解密并去 PKCS7 padding。
pub fn decrypt_cdn_media(data: &[u8], aes_key: &str) -> Result<Vec<u8>, String> {
    let key = parse_aes_key(aes_key)
        .ok_or_else(|| "Weixin attachment AES key is invalid.".to_string())?;
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err("Weixin attachment ciphertext length is invalid.".to_string());
    }
    let mut plaintext = Vec::with_capacity(data.len());
    let (blocks, remainder) = data.as_chunks::<16>();
    if !remainder.is_empty() {
        return Err("Weixin attachment ciphertext length is invalid.".to_string());
    }
    for block in blocks {
        plaintext.extend_from_slice(&decrypt_block(block, &key));
    }
    pkcs7_unpad(plaintext)
}

/// 媒体下载 + 按需解密（`aes_key` 为空表示平台返回的是明文）。
pub async fn download_and_decrypt(
    http: Arc<dyn HttpJsonPort>,
    url: &str,
    aes_key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    if url.trim().is_empty() {
        return Err("Weixin attachment download URL is missing.".to_string());
    }
    let response = http.get_bytes(url, &[], timeout).await?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "Weixin attachment download failed: HTTP {}",
            response.status
        ));
    }
    match aes_key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => decrypt_cdn_media(&response.body, key),
        None => Ok(response.body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_bytes(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn decrypts_nist_ecb_vector() {
        // NIST SP800-38A F.1.2（AES-128-ECB 解密方向）。
        let key: [u8; 16] = hex_bytes("2b7e151628aed2a6abf7158809cf4f3c")
            .try_into()
            .unwrap();
        let ciphertext = hex_bytes("3ad77bb40d7a3660a89ecaf32466ef97");
        let block: [u8; 16] = ciphertext.try_into().unwrap();
        let plaintext = decrypt_block(&block, &key);
        assert_eq!(
            plaintext.to_vec(),
            hex_bytes("6bc1bee22e409f96e93d7e117393172a")
        );
    }

    #[test]
    fn parses_hex_and_base64_keys() {
        let hex = "0123456789abcdef0123456789abcdef";
        let key = parse_aes_key(hex).unwrap();
        assert_eq!(key[0], 0x01);
        assert_eq!(key[15], 0xef);
        // base64(16 字节) 也接受。
        let encoded = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            key,
        );
        assert_eq!(parse_aes_key(&encoded), Some(key));
        assert_eq!(parse_aes_key("short"), None);
    }

    #[test]
    fn decrypt_rejects_invalid_length_key_and_padding() {
        let key = "0123456789abcdef0123456789abcdef";
        assert!(decrypt_cdn_media(&[0u8; 15], key).is_err());
        assert!(decrypt_cdn_media(&[0u8; 16], "not-a-key").is_err());
        // 随机块通常 padding 非法；构造全零密文（解密后 padding 几乎必非法）。
        let randomish: Vec<u8> = (0..32).map(|index| index as u8).collect();
        assert!(decrypt_cdn_media(&randomish, key).is_err());
    }

    struct FakeBytes {
        response: std::sync::Mutex<Option<Result<crate::http::HttpBytesResponse, String>>>,
        requests: std::sync::Mutex<Vec<String>>,
    }

    impl HttpJsonPort for FakeBytes {
        fn post_json<'a>(
            &'a self,
            _request: &'a crate::http::OutboundHttpRequest,
        ) -> crate::http::HttpFuture<'a, Result<crate::http::HttpJsonResponse, String>> {
            Box::pin(async move { Err("not used".to_string()) })
        }

        fn get_bytes<'a>(
            &'a self,
            url: &'a str,
            _headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> crate::http::HttpFuture<'a, Result<crate::http::HttpBytesResponse, String>> {
            self.requests.lock().unwrap().push(url.to_string());
            let response = self
                .response
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Err("no scripted response".to_string()));
            Box::pin(async move { response })
        }
    }

    #[tokio::test]
    async fn download_passthrough_without_aes_key_and_reports_http_errors() {
        let http = std::sync::Arc::new(FakeBytes {
            response: std::sync::Mutex::new(Some(Ok(crate::http::HttpBytesResponse {
                status: 200,
                body: b"plain-bytes".to_vec(),
            }))),
            requests: std::sync::Mutex::new(Vec::new()),
        });
        let bytes = download_and_decrypt(
            http.clone() as std::sync::Arc<dyn HttpJsonPort>,
            "https://cdn.example/file.bin",
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(bytes, b"plain-bytes");
        assert_eq!(
            http.requests.lock().unwrap().as_slice(),
            ["https://cdn.example/file.bin"]
        );

        let http_error = std::sync::Arc::new(FakeBytes {
            response: std::sync::Mutex::new(Some(Ok(crate::http::HttpBytesResponse {
                status: 404,
                body: Vec::new(),
            }))),
            requests: std::sync::Mutex::new(Vec::new()),
        });
        let error = download_and_decrypt(
            http_error as std::sync::Arc<dyn HttpJsonPort>,
            "https://cdn.example/missing",
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(error.contains("HTTP 404"), "{error}");
        assert!(download_and_decrypt(
            std::sync::Arc::new(FakeBytes {
                response: std::sync::Mutex::new(None),
                requests: std::sync::Mutex::new(Vec::new()),
            }),
            " ",
            None,
            Duration::from_secs(5),
        )
        .await
        .is_err());
    }

    #[test]
    fn pkcs7_unpad_accepts_full_block_padding() {
        let mut data = vec![0xAAu8; 32];
        for byte in data.iter_mut().skip(16) {
            *byte = 16;
        }
        assert_eq!(pkcs7_unpad(data).unwrap(), vec![0xAAu8; 16]);
        assert!(pkcs7_unpad(vec![1, 2, 0]).is_err());
        assert!(pkcs7_unpad(Vec::new()).is_err());
    }
}
