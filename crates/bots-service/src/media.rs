//! 出站媒体：路径白名单、类型/限额、分片计划与 MD5。
//!
//! 真源：`packages/services/src/bots/botMedia.ts`（路径/限额）与
//! `@wecom/aibot-node-sdk` 的 `uploadMedia`（512 KiB 分片，最多 100 片）。

use std::path::{Path, PathBuf};

pub const MEDIA_IMAGE_LIMIT_BYTES: u64 = 10 * 1024 * 1024;
pub const MEDIA_FILE_LIMIT_BYTES: u64 = 50 * 1024 * 1024;
pub const MEDIA_CHUNK_BYTES: u64 = 512 * 1024;
pub const MEDIA_MAX_CHUNKS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    File,
}

impl MediaKind {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMediaFile {
    pub real: PathBuf,
    pub filename: String,
    pub kind: MediaKind,
    pub mime_type: String,
    pub size_bytes: u64,
}

pub fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn md5_hex(bytes: &[u8]) -> String {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    for block in message.as_chunks::<64>().0 {
        let mut words = [0u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            *word = u32::from_le_bytes([
                block[index * 4],
                block[index * 4 + 1],
                block[index * 4 + 2],
                block[index * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for index in 0..64 {
            let (f, g) = match index {
                0..=15 => ((b & c) | (!b & d), index),
                16..=31 => ((d & b) | (!d & c), (5 * index + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * index + 5) % 16),
                _ => (c ^ (b | !d), (7 * index) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[index])
                    .wrapping_add(words[g])
                    .rotate_left(SHIFTS[index]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut output = String::with_capacity(32);
    for word in [a0, b0, c0, d0] {
        for byte in word.to_le_bytes() {
            output.push_str(&format!("{byte:02x}"));
        }
    }
    output
}

pub fn media_kind_for(filename: &str) -> (MediaKind, &'static str) {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => (MediaKind::Image, "image/png"),
        "jpg" | "jpeg" => (MediaKind::Image, "image/jpeg"),
        "gif" => (MediaKind::Image, "image/gif"),
        "webp" => (MediaKind::Image, "image/webp"),
        "bmp" => (MediaKind::Image, "image/bmp"),
        "pdf" => (MediaKind::File, "application/pdf"),
        "txt" => (MediaKind::File, "text/plain"),
        "csv" => (MediaKind::File, "text/csv"),
        "json" => (MediaKind::File, "application/json"),
        "zip" => (MediaKind::File, "application/zip"),
        "doc" => (MediaKind::File, "application/msword"),
        "docx" => (
            MediaKind::File,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        "xls" => (MediaKind::File, "application/vnd.ms-excel"),
        "xlsx" => (
            MediaKind::File,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
        "ppt" => (MediaKind::File, "application/vnd.ms-powerpoint"),
        "pptx" => (
            MediaKind::File,
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ),
        _ => (MediaKind::File, "application/octet-stream"),
    }
}

pub fn plan_media_chunks(total_size: u64) -> Result<Vec<(u64, u64)>, String> {
    if total_size == 0 {
        return Err("文件为空，无法发送。".to_string());
    }
    let total_chunks = total_size.div_ceil(MEDIA_CHUNK_BYTES);
    if total_chunks > MEDIA_MAX_CHUNKS {
        return Err(format!(
            "文件超过企微上传上限（{} 个分片，最多 {} 个）。",
            total_chunks, MEDIA_MAX_CHUNKS
        ));
    }
    Ok((0..total_chunks)
        .map(|index| {
            let start = index * MEDIA_CHUNK_BYTES;
            let end = (start + MEDIA_CHUNK_BYTES).min(total_size);
            (start, end)
        })
        .collect())
}

pub fn validate_media_size(kind: MediaKind, size_bytes: u64) -> Result<(), String> {
    let limit = match kind {
        MediaKind::Image => MEDIA_IMAGE_LIMIT_BYTES,
        MediaKind::File => MEDIA_FILE_LIMIT_BYTES,
    };
    if size_bytes > limit {
        return Err(format!(
            "文件过大（{}MB > {}MB）。",
            size_bytes.div_ceil(1024 * 1024),
            limit / 1024 / 1024
        ));
    }
    Ok(())
}

pub fn resolve_media_file(raw_path: &str, roots: &[PathBuf]) -> Result<ResolvedMediaFile, String> {
    let raw_path = raw_path.trim();
    if raw_path.is_empty() {
        return Err("文件路径为空。".to_string());
    }
    let candidate = if Path::new(raw_path).is_absolute() {
        PathBuf::from(raw_path)
    } else {
        let root = roots
            .first()
            .ok_or_else(|| "没有工作区上下文，无法解析相对路径。".to_string())?;
        root.join(raw_path)
    };
    let real = std::fs::canonicalize(&candidate).map_err(|_| format!("文件不存在：{raw_path}"))?;
    let allowed: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect();
    if !allowed.iter().any(|root| real.starts_with(root)) {
        return Err("路径不在允许目录内（工作区 / 应用数据 / 临时目录）。".to_string());
    }
    let metadata = std::fs::metadata(&real).map_err(|_| format!("文件不存在：{raw_path}"))?;
    if !metadata.is_file() {
        return Err("路径不是普通文件。".to_string());
    }
    let filename = real
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| raw_path.to_string());
    let (kind, mime_type) = media_kind_for(&filename);
    validate_media_size(kind, metadata.len())?;
    Ok(ResolvedMediaFile {
        real,
        filename,
        kind,
        mime_type: mime_type.to_string(),
        size_bytes: metadata.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bots-media-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn base64_encodes_upload_chunks() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(&[0, 1, 2, 3]), "AAECAw==");
    }

    #[test]
    fn md5_matches_known_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }

    #[test]
    fn classifies_image_and_file_extensions() {
        assert_eq!(media_kind_for("a.PNG"), (MediaKind::Image, "image/png"));
        assert_eq!(media_kind_for("a.jpg"), (MediaKind::Image, "image/jpeg"));
        assert_eq!(
            media_kind_for("report.pdf"),
            (MediaKind::File, "application/pdf")
        );
        assert_eq!(
            media_kind_for("archive.bin"),
            (MediaKind::File, "application/octet-stream")
        );
    }

    #[test]
    fn plans_chunks_up_to_the_sdk_limit() {
        assert!(plan_media_chunks(0).is_err());
        assert_eq!(plan_media_chunks(1).unwrap(), vec![(0, 1)]);
        assert_eq!(
            plan_media_chunks(MEDIA_CHUNK_BYTES).unwrap(),
            vec![(0, MEDIA_CHUNK_BYTES)]
        );
        assert_eq!(
            plan_media_chunks(MEDIA_CHUNK_BYTES + 1).unwrap(),
            vec![
                (0, MEDIA_CHUNK_BYTES),
                (MEDIA_CHUNK_BYTES, MEDIA_CHUNK_BYTES + 1)
            ]
        );
        assert_eq!(
            plan_media_chunks(MEDIA_CHUNK_BYTES * MEDIA_MAX_CHUNKS)
                .unwrap()
                .len(),
            MEDIA_MAX_CHUNKS as usize
        );
        assert!(plan_media_chunks(MEDIA_CHUNK_BYTES * MEDIA_MAX_CHUNKS + 1).is_err());
    }

    #[test]
    fn enforces_image_and_file_limits() {
        assert!(validate_media_size(MediaKind::Image, MEDIA_IMAGE_LIMIT_BYTES).is_ok());
        assert!(validate_media_size(MediaKind::Image, MEDIA_IMAGE_LIMIT_BYTES + 1).is_err());
        assert!(validate_media_size(MediaKind::File, MEDIA_FILE_LIMIT_BYTES).is_ok());
        assert!(validate_media_size(MediaKind::File, MEDIA_FILE_LIMIT_BYTES + 1).is_err());
    }

    #[test]
    fn resolves_relative_path_inside_the_workspace_root() {
        let root = sandbox("inside");
        let file = root.join("report.txt");
        std::fs::write(&file, b"hello").unwrap();
        let resolved = resolve_media_file("report.txt", std::slice::from_ref(&root)).unwrap();
        assert_eq!(resolved.filename, "report.txt");
        assert_eq!(resolved.kind, MediaKind::File);
        assert_eq!(resolved.size_bytes, 5);
        assert_eq!(resolved.real, std::fs::canonicalize(&file).unwrap());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejects_missing_and_outside_paths() {
        let root = sandbox("roots");
        let outside = sandbox("outside");
        std::fs::write(outside.join("secret.txt"), b"x").unwrap();
        assert!(resolve_media_file("missing.txt", std::slice::from_ref(&root)).is_err());
        let outside_file = outside.join("secret.txt");
        assert!(resolve_media_file(
            outside_file.to_string_lossy().as_ref(),
            std::slice::from_ref(&root)
        )
        .is_err());
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(outside).ok();
    }
}
