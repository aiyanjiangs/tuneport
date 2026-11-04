//! ncm 容器解析与解密。
//!
//! 只做两件事:从文件里读出元信息/密钥/音频区间,以及把音频段流式解密出去。
//! 不负责写文件、不负责标签。

use std::fmt;
use std::io::{Read, Seek, SeekFrom};

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, KeyInit};
use aes::Aes128;

pub const CORE_KEY: [u8; 16] = [
    0x68, 0x7a, 0x48, 0x52, 0x41, 0x6d, 0x73, 0x6f, 0x35, 0x6b, 0x49, 0x6e, 0x62, 0x61, 0x78, 0x57,
];
pub const META_KEY: [u8; 16] = [
    0x23, 0x31, 0x34, 0x6C, 0x6A, 0x6B, 0x5F, 0x21, 0x5C, 0x5D, 0x26, 0x30, 0x55, 0x3C, 0x27, 0x28,
];
const MAGIC: &[u8; 8] = b"CTENFDAM";
const KEY_PREFIX: &[u8] = b"neteasecloudmusic";
const META_PREFIX: &[u8] = b"163 key(Don't modify):";
const META_JSON_PREFIX: &str = "music:";

#[derive(Debug)]
pub enum NcmError {
    /// 不是 ncm(魔数不符)
    Format,
    /// 结构损坏/截断
    Corrupt(String),
    /// 密钥段解不出来
    Key,
    /// 元信息段损坏
    Meta(String),
    /// 新版 QQ 音乐格式:密钥不在文件里,必须联网取
    NeedOnline,
    Io(std::io::Error),
}

impl fmt::Display for NcmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NcmError::Format => write!(f, "不是 ncm 文件(魔数不匹配)"),
            NcmError::Corrupt(s) => write!(f, "文件损坏:{}", s),
            NcmError::Key => write!(f, "无法解出密钥(可能不是标准 ncm)"),
            NcmError::Meta(s) => write!(f, "元信息解析失败:{}", s),
            NcmError::NeedOnline => write!(
                f,
                "这个文件需要联网获取密钥(新版 QQ 音乐格式),本工具不支持离线解密"
            ),
            NcmError::Io(e) => write!(f, "读写失败:{}", e),
        }
    }
}
impl std::error::Error for NcmError {}
impl From<std::io::Error> for NcmError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            NcmError::Corrupt("文件被截断".into())
        } else {
            NcmError::Io(e)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Flac,
    Mp3,
}

impl AudioFormat {
    pub fn ext(&self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "mp3",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub format: String,
    pub raw: serde_json::Value,
}

/// 解析结果
#[derive(Debug)]
pub struct Ncm {
    pub meta: Option<Meta>,
    pub meta_missing: bool,
    pub audio_start: u64,
    pub audio_end: u64,
    pub format: AudioFormat,
    pub key_stream: [u8; 256],
    pub layout: char,
}

fn read_at<R: Read + Seek>(r: &mut R, pos: u64, len: usize) -> Result<Vec<u8>, NcmError> {
    r.seek(SeekFrom::Start(pos))?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn u32_at<R: Read + Seek>(r: &mut R, pos: u64, size: u64) -> Result<Option<u32>, NcmError> {
    if pos + 4 > size {
        return Ok(None);
    }
    let b = read_at(r, pos, 4)?;
    Ok(Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
}

/// AES-128-ECB 解密 + 手动去 PKCS#7 填充(不依赖额外 crate)
pub fn aes128_ecb_decrypt(key: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, NcmError> {
    if data.is_empty() || data.len() % 16 != 0 {
        return Err(NcmError::Corrupt("AES 数据长度异常".into()));
    }
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut out = data.to_vec();
    for chunk in out.chunks_exact_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(chunk));
    }
    let pad = *out.last().unwrap() as usize;
    if pad == 0 || pad > 16 || pad > out.len() {
        return Err(NcmError::Corrupt("PKCS7 填充异常".into()));
    }
    if !out[out.len() - pad..].iter().all(|&b| b as usize == pad) {
        return Err(NcmError::Corrupt("PKCS7 填充不一致".into()));
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

/// 简易 base64 解码(标准字母表,容忍空白)
pub fn base64_decode(s: &str) -> Result<Vec<u8>, NcmError> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = s
        .bytes()
        .filter(|c| !c.is_ascii_whitespace())
        .filter(|&c| c != b'=')
        .collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in clean {
        let v = val(c).ok_or_else(|| NcmError::Meta("base64 含非法字符".into()))?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

/// KSA + PRGA(偏移一位的变体),生成 256 字节密钥流
pub fn build_key_stream(key: &[u8]) -> [u8; 256] {
    let mut box_: [u8; 256] = [0; 256];
    for (i, b) in box_.iter_mut().enumerate() {
        *b = i as u8;
    }
    let mut j: u8 = 0;
    for i in 0..256 {
        j = box_[i].wrapping_add(j).wrapping_add(key[i % key.len()]);
        box_.swap(i, j as usize);
    }
    let mut ks = [0u8; 256];
    for (i, slot) in ks.iter_mut().enumerate() {
        let idx = (i + 1) & 0xff;
        let si = box_[idx];
        let sj = box_[(idx as u8).wrapping_add(si) as usize];
        *slot = box_[si.wrapping_add(sj) as usize];
    }
    ks
}

fn score_audio_head(h: &[u8]) -> u8 {
    if h.len() < 4 {
        return 0;
    }
    if &h[0..4] == b"fLaC" {
        return 5;
    }
    if &h[0..3] == b"ID3" {
        return 5;
    }
    if h[0] == 0xff && (h[1] & 0xe0) == 0xe0 {
        return 3; // MPEG/AAC 帧同步
    }
    0
}

fn parse_meta(bytes: &[u8]) -> Result<Meta, NcmError> {
    let mut v = bytes.to_vec();
    for b in v.iter_mut() {
        *b ^= 0x63;
    }
    if v.len() <= META_PREFIX.len() || &v[..META_PREFIX.len()] != META_PREFIX {
        return Err(NcmError::Meta("缺少 163 key 前缀".into()));
    }
    let b64 = String::from_utf8_lossy(&v[META_PREFIX.len()..]).to_string();
    let cipher = base64_decode(b64.trim())?;
    let plain = aes128_ecb_decrypt(&META_KEY, &cipher)?;
    let text = String::from_utf8_lossy(&plain);
    let json = text
        .strip_prefix(META_JSON_PREFIX)
        .ok_or_else(|| NcmError::Meta("缺少 music: 前缀".into()))?;
    let raw: serde_json::Value = serde_json::from_str(json).map_err(|e| NcmError::Meta(e.to_string()))?;

    let title = raw
        .get("musicName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let album = raw.get("album").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let format = raw.get("format").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let artist = raw
        .get("artist")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|a| match a {
                    serde_json::Value::Array(items) => items
                        .first()
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    _ => String::new(),
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();

    Ok(Meta { title, artist, album, format, raw })
}

/// 解析 ncm 头部。`size` 是文件总长度。
pub fn inspect<R: Read + Seek>(r: &mut R, size: u64) -> Result<Ncm, NcmError> {
    if size < 32 {
        return Err(NcmError::Corrupt("文件太小,不是有效的 ncm".into()));
    }
    let head = read_at(r, 0, 8)?;
    if head != MAGIC {
        return Err(NcmError::Format);
    }

    let mut pos: u64 = 10; // 8 字节魔数 + 2 字节间隔

    // ---- RC4 密钥段 ----
    let key_len = u32_at(r, pos, size)?.unwrap_or(0);
    pos += 4;
    if key_len == 0 || key_len > 1 << 20 || pos + key_len as u64 > size {
        return Err(NcmError::Corrupt("密钥段长度异常".into()));
    }
    let mut key_cipher = read_at(r, pos, key_len as usize)?;
    for b in key_cipher.iter_mut() {
        *b ^= 0x64;
    }
    pos += key_len as u64;

    let key_plain = aes128_ecb_decrypt(&CORE_KEY, &key_cipher)?;
    if key_plain.len() <= KEY_PREFIX.len() || &key_plain[..KEY_PREFIX.len()] != KEY_PREFIX {
        return Err(NcmError::Key);
    }
    let key_stream = build_key_stream(&key_plain[KEY_PREFIX.len()..]);

    // ---- 元信息段 ----
    let meta_len = u32_at(r, pos, size)?.unwrap_or(0);
    pos += 4;
    let mut meta = None;
    let mut meta_missing = false;
    if meta_len == 0 {
        meta_missing = true;
    } else {
        if pos + meta_len as u64 > size {
            return Err(NcmError::Corrupt("元信息段越界".into()));
        }
        let meta_bytes = read_at(r, pos, meta_len as usize)?;
        pos += meta_len as u64;
        if !meta_bytes.is_empty() {
            meta = Some(parse_meta(&meta_bytes)?);
        }
    }

    // ---- 定位音频起点(三种封面段布局投票)----
    let len_a = u32_at(r, pos + 5, size)?.unwrap_or(0);
    let len_b = u32_at(r, pos + 9, size)?.unwrap_or(0);

    let candidates: Vec<(char, u64)> = vec![
        ('A', pos + 9 + len_a as u64),
        ('B', pos + 13 + len_b as u64),
        ('C', pos + 13 + len_a as u64),
    ]
    .into_iter()
    .filter(|(_, start)| *start >= pos && start + 4 <= size)
    .collect();

    let mut best: Option<(char, u64, u8)> = None;
    let mut tried = Vec::new();
    for (layout, start) in candidates {
        let raw = read_at(r, start, std::cmp::min(8, (size - start) as usize))?;
        let mut h = raw;
        for (i, b) in h.iter_mut().enumerate() {
            *b ^= key_stream[i & 0xff];
        }
        let score = score_audio_head(&h);
        tried.push(format!("{}@{}", layout, start));
        if best.map_or(true, |(_, _, s)| score > s) {
            best = Some((layout, start, score));
        }
    }

    let (layout, audio_start, _score) = best.ok_or_else(|| {
        NcmError::Corrupt(format!("无法定位音频数据(尝试:{})", tried.join(", ")))
    })?;
    if _score == 0 {
        return Err(NcmError::Corrupt(format!(
            "无法定位音频数据(尝试:{})",
            tried.join(", ")
        )));
    }

    let format = match meta.as_ref().map(|m| m.format.to_lowercase()).as_deref() {
        Some("flac") => AudioFormat::Flac,
        Some("mp3") => AudioFormat::Mp3,
        _ => {
            let h = read_at(r, audio_start, std::cmp::min(4, (size - audio_start) as usize))?;
            let mut h = h;
            for (i, b) in h.iter_mut().enumerate() {
                *b ^= key_stream[i & 0xff];
            }
            if h.len() >= 4 && &h[0..4] == b"fLaC" {
                AudioFormat::Flac
            } else {
                AudioFormat::Mp3
            }
        }
    };

    Ok(Ncm {
        meta,
        meta_missing,
        audio_start,
        audio_end: size,
        format,
        key_stream,
        layout,
    })
}

/// 流式解密 [start, end),逐块交给 sink。`origin` 是密钥流的对齐起点(通常是音频起点)。
pub fn decode_audio<R, F>(
    r: &mut R,
    start: u64,
    end: u64,
    key_stream: &[u8; 256],
    origin: u64,
    mut sink: F,
) -> Result<u64, NcmError>
where
    R: Read + Seek,
    F: FnMut(&[u8]) -> Result<(), NcmError>,
{
    const CHUNK: usize = 1 << 20;
    let mut pos = start;
    let mut total = 0u64;
    while pos < end {
        let n = std::cmp::min(CHUNK as u64, end - pos) as usize;
        let mut buf = read_at(r, pos, n)?;
        for (i, b) in buf.iter_mut().enumerate() {
            let off = (pos - origin) as usize + i;
            *b ^= key_stream[off & 0xff];
        }
        sink(&buf)?;
        total += n as u64;
        pos += n as u64;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_basic() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVs bG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn aes_pkcs7_roundtrip() {
        // 用已知向量:对 "neteasecloudmusic" 加密再解密
        use aes::cipher::BlockEncrypt;
        let cipher = Aes128::new(GenericArray::from_slice(&CORE_KEY));
        let mut block = [0u8; 32];
        block[..17].copy_from_slice(KEY_PREFIX);
        let pad = 32 - 17;
        for b in block[17..].iter_mut() {
            *b = pad as u8;
        }
        for c in block.chunks_exact_mut(16) {
            cipher.encrypt_block(GenericArray::from_mut_slice(c));
        }
        let plain = aes128_ecb_decrypt(&CORE_KEY, &block).unwrap();
        assert_eq!(&plain[..17], KEY_PREFIX);
        assert_eq!(plain.len(), 17);
    }
}
