//! QQ 音乐加密格式(.qmc* / .mflac / .mgg)的解密。
//!
//! 两套方案:
//! - **QMC1**(旧,`.qmc0/.qmc2/.qmc3/.qmcflac/.qmcogg`):固定 64 字节表按偏移异或,无需密钥。
//! - **QMC2**(新,`.mflac/.mgg` 等):文件尾部藏着 ekey,先 base64 解码再 TC-TEA 解出真实密钥,
//!   音频用"映射密码"(密钥 ≤300 字节)或"改版 RC4"(密钥 >300 字节)加密。
//!
//! 注意:QQ 音乐 ≥ 19.57 用的 musicex 尾部**不在文件里存密钥**,必须联网向服务器取,
//! 这种情况本工具无法离线解密,会给出明确提示。

use crate::ncm::base64_decode;

/// 支持解密的扩展名(小写)
pub const QMC_EXTS: &[&str] = &[
    "qmc0", "qmc2", "qmc3", "qmcflac", "qmcogg", "qmc", "mgg", "mgg0", "mgg1", "mggl", "mflac",
    "mflac0", "mflach",
];

pub fn is_qmc_ext(ext: &str) -> bool {
    let e = ext.to_ascii_lowercase();
    QMC_EXTS.contains(&e.as_str())
}

/// 解不出来时的兜底扩展名(能探测出真实格式时以探测结果为准)
pub fn fallback_ext(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "qmcflac" | "mflac" | "mflac0" | "mflach" => "flac",
        "qmcogg" | "mgg" | "mgg0" | "mgg1" | "mggl" | "qmc2" => "ogg",
        _ => "mp3",
    }
}

/// 按解密后的文件头判断真实音频格式
pub fn detect_audio(buf: &[u8]) -> Option<&'static str> {
    if buf.len() >= 4 && &buf[..4] == b"fLaC" {
        return Some("flac");
    }
    if buf.len() >= 4 && &buf[..4] == b"OggS" {
        return Some("ogg");
    }
    if buf.len() >= 3 && &buf[..3] == b"ID3" {
        return Some("mp3");
    }
    if buf.len() >= 2 && buf[0] == 0xff && (buf[1] & 0xe0) == 0xe0 {
        return Some("mp3");
    }
    if buf.len() >= 12 && &buf[..4] == b"RIFF" && &buf[8..12] == b"WAVE" {
        return Some("wav");
    }
    None
}

/// 标准 base64 编码(V1 尾部要把原始密钥再编码回 ekey)
pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        out.push(T[(b[0] >> 2) as usize] as char);
        out.push(T[(((b[0] & 3) << 4) | (b[1] >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(((b[1] & 0xf) << 2) | (b[2] >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(b[2] & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// QMC1 的 64 字节固定表(由 8×7 种子表按 Z 字形遍历生成,是格式常量)
const QMC1_TABLE: [u8; 64] = [
    0xc3, 0x4a, 0xd6, 0xca, 0x90, 0x67, 0xf7, 0x52, 0xd8, 0xa1, 0x66, 0x62, 0x9f, 0x5b, 0x09, 0x00,
    0xc3, 0x5e, 0x95, 0x23, 0x9f, 0x13, 0x11, 0x7e, 0xd8, 0x92, 0x3f, 0xbc, 0x90, 0xbb, 0x74, 0x0e,
    0xc3, 0x47, 0x74, 0x3d, 0x90, 0xaa, 0x3f, 0x51, 0xd8, 0xf4, 0x11, 0x84, 0x9f, 0xde, 0x95, 0x1d,
    0xc3, 0xc6, 0x09, 0xd5, 0x9f, 0xfa, 0x66, 0xf9, 0xd8, 0xf0, 0xf7, 0xa0, 0x90, 0xa1, 0xd6, 0xf3,
];

#[inline]
fn qmc1_mask(offset: usize) -> u8 {
    let idx = (offset % 0x7fff) & 0x7f;
    let idx = if idx > 0x3f { (0x80 - idx) & 0x3f } else { idx };
    QMC1_TABLE[idx]
}

/// 文件尾部类型
#[derive(Debug, Clone, PartialEq)]
pub enum Footer {
    /// 末尾 4 字节 = 密钥长度(LE),密钥紧挨在前面
    V1 { key_size: usize },
    /// 末尾 4 字节是 "QTag",前面是 "ekey,songid," 形式的元数据
    QTag { ekey: String },
    /// 最新格式:尾部只有歌 ID / mid / 文件名,**不含密钥**
    Musicex { song_id: u32, mid: String },
    Unknown,
}

/// 从文件末尾识别尾部类型
pub fn detect_footer(data: &[u8]) -> Footer {
    if data.len() < 8 {
        return Footer::Unknown;
    }
    let n = data.len();

    // musicex:末尾 8 字节是 "musicex\0"
    if n >= 16 && &data[n - 8..] == b"musicex\x00" {
        let version = u32::from_le_bytes(data[n - 12..n - 8].try_into().unwrap_or([0; 4]));
        let footer_size = u32::from_le_bytes(data[n - 16..n - 12].try_into().unwrap_or([0; 4]));
        if version == 1 && footer_size as usize >= 16 && (footer_size as usize) <= n {
            let start = n - footer_size as usize;
            let meta = &data[start..n - 16];
            let song_id = if meta.len() >= 4 {
                u32::from_le_bytes(meta[0..4].try_into().unwrap_or([0; 4]))
            } else {
                0
            };
            return Footer::Musicex {
                song_id,
                mid: read_utf16le(meta, 0x0c, 60),
            };
        }
    }

    // QTag:末尾 4 字节
    if u32::from_le_bytes(data[n - 4..].try_into().unwrap_or([0; 4])) == 0x6761_5451 && n >= 12 {
        let meta_size = u32::from_be_bytes(data[n - 8..n - 4].try_into().unwrap_or([0; 4])) as usize;
        let end = n - 8;
        if meta_size <= end {
            let meta = &data[end - meta_size..end];
            // "ekey,songid," 形式,取第一段
            let ekey = match meta.iter().position(|&b| b == b',') {
                Some(p) => String::from_utf8_lossy(&meta[..p]).to_string(),
                None => String::from_utf8_lossy(meta).to_string(),
            };
            return Footer::QTag { ekey };
        }
    }

    // V1:末尾 4 字节是密钥长度
    let key_size = u32::from_le_bytes(data[n - 4..].try_into().unwrap_or([0; 4])) as usize;
    if key_size > 0 && key_size <= 0x400 && key_size + 4 <= n {
        return Footer::V1 { key_size };
    }

    Footer::Unknown
}

/// 读取 UTF-16LE 字符串(遇到 0 结束)
fn read_utf16le(data: &[u8], offset: usize, max_len: usize) -> String {
    let end = (offset + max_len).min(data.len());
    let mut units = Vec::new();
    let mut i = offset;
    while i + 1 < end {
        let u = u16::from_le_bytes([data[i], data[i + 1]]);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    String::from_utf16_lossy(&units)
}

/// 解密器:按偏移可寻址,支持流式分块
pub enum Cipher {
    /// QMC1:固定表
    Static,
    /// QMC2 映射密码(密钥 ≤ 300 字节)
    Map { key: Vec<u8> },
    /// QMC2 改版 RC4(密钥 > 300 字节)
    Rc4 {
        s: Vec<u8>,
        hash: u64,
        key: Vec<u8>,
    },
}

impl Cipher {
    /// QMC1
    pub fn static_cipher() -> Self {
        Cipher::Static
    }

    /// 从 ekey 字符串构造 QMC2 解密器
    pub fn from_ekey(ekey: &str) -> Result<Self, String> {
        let key = parse_ekey(ekey)?;
        if key.len() > 300 {
            Ok(Cipher::Rc4 {
                s: rc4_init(&key),
                hash: rc4_hash_base(&key),
                key,
            })
        } else {
            Ok(Cipher::Map { key })
        }
    }

    pub fn decrypt(&self, offset: usize, buf: &mut [u8]) {
        match self {
            Cipher::Static => {
                for (i, b) in buf.iter_mut().enumerate() {
                    *b ^= qmc1_mask(offset + i);
                }
            }
            Cipher::Map { key } => {
                for (i, b) in buf.iter_mut().enumerate() {
                    *b ^= map_mask(key, offset + i);
                }
            }
            Cipher::Rc4 { s, hash, key } => rc4_decrypt(s, *hash, key, offset, buf),
        }
    }
}

/* ---------------- QMC2 密钥派生 ---------------- */

const ENCV2_PREFIX: &[u8] = b"QQMusic EncV2,Key:";
const ENCV2_KEY1: &[u8] = b"386ZJY!@#*$%^&)(";
const ENCV2_KEY2: &[u8] = b"**#!(#$%&^a1cZ,T";

/// 把 ekey 还原成真正的解密密钥
pub fn parse_ekey(ekey: &str) -> Result<Vec<u8>, String> {
    let trimmed: String = ekey.chars().filter(|c| *c != '\0').collect();
    let decoded = base64_decode(trimmed.trim()).map_err(|_| "ekey 不是合法的 base64".to_string())?;
    if decoded.is_empty() {
        return Err("ekey 为空".into());
    }

    let decoded = if decoded.starts_with(ENCV2_PREFIX) {
        let blob = &decoded[ENCV2_PREFIX.len()..];
        let stage1 = tc_tea::decrypt(blob, ENCV2_KEY1).map_err(|_| "ekey 解码失败(阶段 1)")?;
        let stage2 = tc_tea::decrypt(&stage1, ENCV2_KEY2).map_err(|_| "ekey 解码失败(阶段 2)")?;
        let text = String::from_utf8_lossy(&stage2).to_string();
        base64_decode(text.trim()).map_err(|_| "EncV2 内层 base64 非法".to_string())?
    } else {
        decoded
    };

    if decoded.len() < 8 {
        return Err("密钥太短".into());
    }

    let (header, body) = decoded.split_at(8);
    if body.is_empty() {
        return Ok(header.to_vec());
    }

    // EncV1:前 8 字节是头,用它派生出 TEA 密钥,再解主体
    let tea_key = derive_tea_key(header);
    match tc_tea::decrypt(body, &tea_key) {
        Ok(plain) => {
            let mut out = Vec::with_capacity(8 + plain.len());
            out.extend_from_slice(header);
            out.extend_from_slice(&plain);
            Ok(out)
        }
        // 解不开说明是 API 直接给的裸密钥,原样使用
        Err(_) => Ok(decoded),
    }
}

/// 派生 TEA 密钥:交错固定序列与 ekey 头
fn derive_tea_key(header: &[u8]) -> [u8; 16] {
    let fixed = simple_make_key(106);
    let mut key = [0u8; 16];
    for i in 0..8 {
        key[i * 2] = fixed[i];
        key[i * 2 + 1] = header[i];
    }
    key
}

/// 固定扰动序列(tan 取绝对值截断,和客户端一致)
fn simple_make_key(seed: u8) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, slot) in out.iter_mut().enumerate() {
        let v = seed as f32 + i as f32 * 0.1;
        *slot = (100.0 * v.tan().abs()) as u8;
    }
    out
}

/* ---------------- QMC2 映射密码 ---------------- */

#[inline]
fn scramble(value: u8, index: usize) -> u8 {
    let rot = ((index as u32) + 4) & 7;
    value.wrapping_shl(rot) | value.wrapping_shr(rot)
}

#[inline]
fn map_mask(key: &[u8], offset: usize) -> u8 {
    let mut o = offset;
    if o > 0x7fff {
        o %= 0x7fff;
    }
    if key.is_empty() {
        return 0;
    }
    let idx = (o * o + 71214) % key.len();
    scramble(key[idx], idx)
}

/* ---------------- QMC2 改版 RC4 ---------------- */

const FIRST_SEG: usize = 0x80;
const OTHER_SEG: usize = 0x1400;

fn rc4_init(key: &[u8]) -> Vec<u8> {
    let n = key.len();
    let mut s: Vec<u8> = if n <= 256 {
        (0..n as u8).collect()
    } else {
        let mut v: Vec<u8> = (0..=255u8).collect();
        v.extend((0..=255u8).cycle().take(n - 256));
        v
    };
    let mut j = 0usize;
    for i in 0..n {
        j = (j + s[i] as usize + key[i] as usize) % n;
        s.swap(i, j);
    }
    s
}

fn rc4_hash_base(key: &[u8]) -> u64 {
    let mut hash: u64 = 1;
    for &b in key {
        let v = b as u64;
        if v == 0 {
            continue;
        }
        let next = hash.wrapping_mul(v);
        if next == 0 || next <= hash {
            break;
        }
        hash = next;
    }
    hash
}

#[inline]
fn seg_key(hash: u64, id: usize, seed: u8) -> usize {
    let divisor = ((id + 1) * seed as usize) as f64;
    if divisor == 0.0 {
        return 0;
    }
    (hash as f64 / divisor * 100.0) as u64 as usize
}

#[inline]
fn rc4_derive(n: usize, s: &mut [u8], j: &mut usize, k: &mut usize) -> u8 {
    *j = (*j + 1) % n;
    *k = (s[*j] as usize + *k) % n;
    s.swap(*j, *k);
    s[(s[*j] as usize + s[*k] as usize) % n]
}

fn rc4_first_seg(key: &[u8], hash: u64, offset: usize, buf: &mut [u8]) {
    let n = key.len();
    let mut off = offset;
    for b in buf.iter_mut() {
        let seed = key[off % n];
        let idx = seg_key(hash, off, seed);
        *b ^= key[idx % n];
        off += 1;
    }
}

fn rc4_other_seg(s0: &[u8], hash: u64, key: &[u8], offset: usize, buf: &mut [u8]) {
    let n = key.len();
    let seg = offset / OTHER_SEG;
    let mut discard = (seg_key(hash, seg, key[seg & 0x1ff]) & 0x1ff) + offset % OTHER_SEG;
    let mut s = s0.to_vec();
    let (mut j, mut k) = (0usize, 0usize);
    while discard > 0 {
        rc4_derive(n, &mut s, &mut j, &mut k);
        discard -= 1;
    }
    for b in buf.iter_mut() {
        *b ^= rc4_derive(n, &mut s, &mut j, &mut k);
    }
}

fn rc4_decrypt(s0: &[u8], hash: u64, key: &[u8], offset: usize, buf: &mut [u8]) {
    let mut off = offset;
    let mut i = 0usize;
    let mut left = buf.len();

    if off < FIRST_SEG {
        let take = left.min(FIRST_SEG - off);
        rc4_first_seg(key, hash, off, &mut buf[i..i + take]);
        i += take;
        left -= take;
        off += take;
    }

    let misaligned = off % OTHER_SEG;
    if misaligned != 0 && left > 0 {
        let take = left.min(OTHER_SEG - misaligned);
        rc4_other_seg(s0, hash, key, off, &mut buf[i..i + take]);
        i += take;
        left -= take;
        off += take;
    }

    while left > OTHER_SEG {
        rc4_other_seg(s0, hash, key, off, &mut buf[i..i + OTHER_SEG]);
        i += OTHER_SEG;
        left -= OTHER_SEG;
        off += OTHER_SEG;
    }

    if left > 0 {
        rc4_other_seg(s0, hash, key, off, &mut buf[i..i + left]);
    }
}

/* ---------------- 从解密后的音频里读标签 ---------------- */

#[derive(Debug, Default, Clone)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

impl Tags {
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.artist.is_none() && self.album.is_none()
    }
}

/// 尽量读出标题/歌手/专辑(FLAC 的 Vorbis Comment、MP3 的 ID3v2、OGG 的 Vorbis/Opus 头)
pub fn read_tags(buf: &[u8]) -> Tags {
    // 有些格式(酷我)解密后开头是一段静音,先跳过
    let start = buf.iter().position(|b| *b != 0).unwrap_or(buf.len());
    let buf = &buf[start..];
    if buf.len() > 4 && &buf[..4] == b"fLaC" {
        return read_flac_tags(buf);
    }
    if buf.len() > 10 && &buf[..3] == b"ID3" {
        return read_id3_tags(buf);
    }
    if buf.len() > 4 && &buf[..4] == b"OggS" {
        return read_ogg_tags(buf);
    }
    Tags::default()
}

/// OGG 的注释头是第 2 个包,可能跨页,所以先把前两个包拼出来
fn read_ogg_tags(buf: &[u8]) -> Tags {
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    while pos + 27 <= buf.len() && &buf[pos..pos + 4] == b"OggS" {
        let nseg = buf[pos + 26] as usize;
        if pos + 27 + nseg > buf.len() {
            break;
        }
        let mut dpos = pos + 27 + nseg;
        for i in 0..nseg {
            let n = buf[pos + 27 + i] as usize;
            if dpos + n > buf.len() {
                return Tags::default();
            }
            cur.extend_from_slice(&buf[dpos..dpos + n]);
            dpos += n;
            if n < 255 {
                packets.push(std::mem::take(&mut cur));
                if packets.len() >= 2 {
                    break;
                }
            }
        }
        if packets.len() >= 2 {
            break;
        }
        pos = dpos;
    }
    if packets.len() < 2 {
        return Tags::default();
    }
    let p = &packets[1];
    let body = if p.starts_with(b"\x03vorbis") {
        &p[7..]
    } else if p.starts_with(b"OpusTags") {
        &p[8..]
    } else {
        return Tags::default();
    };
    parse_vorbis_comment(body)
}

fn read_flac_tags(buf: &[u8]) -> Tags {
    let mut tags = Tags::default();
    let mut pos = 4usize;
    while pos + 4 <= buf.len() {
        let header = buf[pos];
        let is_last = header & 0x80 != 0;
        let block_type = header & 0x7f;
        let len = ((buf[pos + 1] as usize) << 16)
            | ((buf[pos + 2] as usize) << 8)
            | buf[pos + 3] as usize;
        pos += 4;
        if pos + len > buf.len() {
            break;
        }
        if block_type == 4 {
            tags = parse_vorbis_comment(&buf[pos..pos + len]);
            break;
        }
        pos += len;
        if is_last {
            break;
        }
    }
    tags
}

fn parse_vorbis_comment(data: &[u8]) -> Tags {
    let mut tags = Tags::default();
    if data.len() < 4 {
        return tags;
    }
    let vendor_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let mut pos = 4 + vendor_len;
    if pos + 4 > data.len() {
        return tags;
    }
    let count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;
    for _ in 0..count {
        if pos + 4 > data.len() {
            break;
        }
        let len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + len > data.len() {
            break;
        }
        let entry = String::from_utf8_lossy(&data[pos..pos + len]).to_string();
        pos += len;
        if let Some((k, v)) = entry.split_once('=') {
            let v = v.trim().to_string();
            if v.is_empty() {
                continue;
            }
            match k.to_ascii_uppercase().as_str() {
                "TITLE" => tags.title = Some(v),
                "ARTIST" => tags.artist = Some(v),
                "ALBUM" => tags.album = Some(v),
                _ => {}
            }
        }
    }
    tags
}

fn read_id3_tags(buf: &[u8]) -> Tags {
    let mut tags = Tags::default();
    let ver = buf[3];
    let size = ((buf[6] as usize) << 21)
        | ((buf[7] as usize) << 14)
        | ((buf[8] as usize) << 7)
        | buf[9] as usize;
    let end = (10 + size).min(buf.len());
    let mut pos = 10usize;
    // 扩展头
    if ver >= 3 && buf.get(5).copied().unwrap_or(0) & 0x40 != 0 && pos + 4 <= end {
        let ext = ((buf[pos] as usize) << 24)
            | ((buf[pos + 1] as usize) << 16)
            | ((buf[pos + 2] as usize) << 8)
            | buf[pos + 3] as usize;
        pos += 4 + ext;
    }
    while pos + 10 <= end {
        let id = &buf[pos..pos + 4];
        if id[0] == 0 {
            break;
        }
        let fsize = if ver >= 4 {
            ((buf[pos + 4] as usize) << 21)
                | ((buf[pos + 5] as usize) << 14)
                | ((buf[pos + 6] as usize) << 7)
                | buf[pos + 7] as usize
        } else {
            ((buf[pos + 4] as usize) << 24)
                | ((buf[pos + 5] as usize) << 16)
                | ((buf[pos + 6] as usize) << 8)
                | buf[pos + 7] as usize
        };
        pos += 10;
        if pos + fsize > end {
            break;
        }
        let body = &buf[pos..pos + fsize];
        pos += fsize;
        let text = decode_text_frame(body);
        match id {
            b"TIT2" => tags.title = text,
            b"TPE1" => tags.artist = text,
            b"TALB" => tags.album = text,
            _ => {}
        }
    }
    tags
}

fn decode_text_frame(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let enc = body[0];
    let data = &body[1..];
    let s = match enc {
        0 => data.iter().map(|b| *b as char).collect::<String>(),
        1 => {
            // UTF-16 带 BOM
            if data.len() < 2 {
                return None;
            }
            let le = data[0] == 0xff && data[1] == 0xfe;
            let units: Vec<u16> = data[2..]
                .chunks_exact(2)
                .map(|c| {
                    if le {
                        u16::from_le_bytes([c[0], c[1]])
                    } else {
                        u16::from_be_bytes([c[0], c[1]])
                    }
                })
                .take_while(|u| *u != 0)
                .collect();
            String::from_utf16_lossy(&units)
        }
        2 => data
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .take_while(|u| *u != 0)
            .map(|u| char::from_u32(u as u32).unwrap_or('\u{fffd}'))
            .collect::<String>(),
        _ => String::from_utf8_lossy(data).to_string(),
    };
    let s = s.trim().trim_matches('\0').trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qmc1_mask_matches_reference() {
        assert_eq!(qmc1_mask(0), 0xc3);
        assert_eq!(qmc1_mask(1), 0x4a);
        assert_eq!(qmc1_mask(63), 0xf3);
        // 索引超过 0x3f 后反射回前半段
        assert_eq!(qmc1_mask(0x40), QMC1_TABLE[0x00]);
        assert_eq!(qmc1_mask(0x7f), QMC1_TABLE[0x01]);
        // 0x7fff 是周期边界
        assert_eq!(qmc1_mask(0x7fff), qmc1_mask(0));
    }

    #[test]
    fn qmc1_roundtrip() {
        let plain: Vec<u8> = (0..10_000u32).map(|i| (i * 37 % 251) as u8).collect();
        let mut data = plain.clone();
        Cipher::Static.decrypt(0, &mut data);
        assert_ne!(data, plain);
        Cipher::Static.decrypt(0, &mut data);
        assert_eq!(data, plain);
    }

    #[test]
    fn qmc2_map_matches_reference() {
        // 参考实现给的向量:key=0x41..0x50,16 个 0 字节
        let key: Vec<u8> = (0x41u8..=0x50).collect();
        let c = Cipher::Map { key };
        let mut data = [0u8; 16];
        c.decrypt(0, &mut data);
        assert_eq!(
            data,
            [
                0x3f, 0x8a, 0xc1, 0x49, 0x3f, 0x49, 0xc1, 0x8a, 0x3f, 0x8a, 0xc1, 0x49, 0x3f,
                0x49, 0xc1, 0x8a
            ]
        );
    }

    #[test]
    fn qmc2_rc4_first_segment_matches_reference() {
        let key: Vec<u8> = (0..255u8).collect();
        let s = rc4_init(&key);
        let hash = rc4_hash_base(&key);
        let c = Cipher::Rc4 { s, hash, key };
        let mut data = [0u8; 16];
        c.decrypt(0, &mut data);
        assert_eq!(data, [0, 50, 16, 8, 5, 3, 2, 1, 1, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn simple_make_key_matches_reference() {
        assert_eq!(simple_make_key(106), [0x69, 0x56, 0x46, 0x38, 0x2b, 0x20, 0x15, 0x0b]);
    }

    #[test]
    fn derive_tea_key_interleaves() {
        let key = derive_tea_key(&[0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8]);
        assert_eq!(
            key,
            [0x69, 0xf1, 0x56, 0xf2, 0x46, 0xf3, 0x38, 0xf4, 0x2b, 0xf5, 0x20, 0xf6, 0x15, 0xf7, 0x0b, 0xf8]
        );
    }

    #[test]
    fn parse_ekey_raw_key() {
        // 参考向量:base64 解出来是明文,不是 EncV1/EncV2
        let ekey = "VGhpcyBpcyBhIHRlc3Qga2V5IGZvciB0ZXN0IHB1cnBvc2UgOkQ=";
        let key = parse_ekey(ekey).unwrap();
        assert_eq!(key, b"This is a test key for test purpose :D");
    }

    #[test]
    fn parse_ekey_encv1_roundtrip() {
        // EncV1:前 8 字节头 + TEA 加密的主体
        let plain = b"12345678...test data by Jixun";
        let (header, body) = plain.split_at(8);
        let tea_key = derive_tea_key(header);
        let enc = tc_tea::encrypt(body, &tea_key).unwrap();
        let mut blob = header.to_vec();
        blob.extend_from_slice(&enc);
        let ekey = base64_encode(&blob);
        assert_eq!(parse_ekey(&ekey).unwrap(), plain);
    }

    #[test]
    fn footer_v1_detected() {
        let mut data = vec![0u8; 100];
        let key = b"some-ekey-bytes";
        data.extend_from_slice(key);
        data.extend_from_slice(&(key.len() as u32).to_le_bytes());
        assert_eq!(detect_footer(&data), Footer::V1 { key_size: key.len() });
    }

    #[test]
    fn footer_qtag_detected() {
        let meta = b"ABCekey,123456,";
        let mut data = vec![7u8; 50];
        data.extend_from_slice(meta);
        data.extend_from_slice(&(meta.len() as u32).to_be_bytes());
        data.extend_from_slice(b"QTag");
        assert_eq!(detect_footer(&data), Footer::QTag { ekey: "ABCekey".into() });
    }

    #[test]
    fn footer_musicex_detected() {
        let mut data = vec![1u8; 64];
        let footer_size = 16u32 + 64u32;
        let mut meta = vec![0u8; 64];
        meta[0..4].copy_from_slice(&4321u32.to_le_bytes());
        data.extend_from_slice(&meta);
        data.extend_from_slice(&footer_size.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(b"musicex\x00");
        match detect_footer(&data) {
            Footer::Musicex { song_id, .. } => assert_eq!(song_id, 4321),
            other => panic!("应识别为 musicex,实际 {:?}", other),
        }
    }

    #[test]
    fn flac_tags_parsed() {
        let mut vc = Vec::new();
        vc.extend_from_slice(&0u32.to_le_bytes()); // vendor 空
        vc.extend_from_slice(&2u32.to_le_bytes());
        for (k, v) in [("TITLE", "晴天"), ("ARTIST", "周杰伦")] {
            let s = format!("{}={}", k, v);
            vc.extend_from_slice(&(s.len() as u32).to_le_bytes());
            vc.extend_from_slice(s.as_bytes());
        }
        let mut buf = b"fLaC".to_vec();
        buf.push(0x80 | 4); // 最后一个块,类型 4
        buf.extend_from_slice(&(vc.len() as u32).to_be_bytes()[1..]);
        buf.extend_from_slice(&vc);
        let tags = read_tags(&buf);
        assert_eq!(tags.title.as_deref(), Some("晴天"));
        assert_eq!(tags.artist.as_deref(), Some("周杰伦"));
    }

    #[test]
    fn id3_tags_parsed() {
        let mut frames = Vec::new();
        for (id, text) in [(&b"TIT2"[..], "夜曲"), (&b"TPE1"[..], "周杰伦")] {
            let mut body = vec![1u8]; // UTF-16
            body.extend_from_slice(&[0xff, 0xfe]);
            for u in text.encode_utf16() {
                body.extend_from_slice(&u.to_le_bytes());
            }
            frames.extend_from_slice(id);
            frames.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frames.extend_from_slice(&[0, 0]);
            frames.extend_from_slice(&body);
        }
        let mut buf = b"ID3\x03\x00\x00".to_vec();
        let size = frames.len();
        buf.extend_from_slice(&[
            ((size >> 21) & 0x7f) as u8,
            ((size >> 14) & 0x7f) as u8,
            ((size >> 7) & 0x7f) as u8,
            (size & 0x7f) as u8,
        ]);
        buf.extend_from_slice(&frames);
        let tags = read_tags(&buf);
        assert_eq!(tags.title.as_deref(), Some("夜曲"));
        assert_eq!(tags.artist.as_deref(), Some("周杰伦"));
    }

    #[test]
    fn ogg_vorbis_tags_parsed() {
        // 造两页 ogg:第 1 包是识别头,第 2 包是注释头
        fn page(seq: u8, packets: &[Vec<u8>]) -> Vec<u8> {
            let mut segs = Vec::new();
            let mut body = Vec::new();
            for p in packets {
                let mut left = p.len();
                while left >= 255 {
                    segs.push(255u8);
                    left -= 255;
                }
                segs.push(left as u8);
                body.extend_from_slice(p);
            }
            let mut out = b"OggS".to_vec();
            out.push(0); // version
            out.push(0); // header type
            out.extend_from_slice(&0u64.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
            out.extend_from_slice(&(seq as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.push(segs.len() as u8);
            out.extend_from_slice(&segs);
            out.extend_from_slice(&body);
            out
        }

        let ident = b"\x01vorbis".to_vec();
        let mut comment = b"\x03vorbis".to_vec();
        comment.extend_from_slice(&0u32.to_le_bytes()); // vendor 空
        comment.extend_from_slice(&2u32.to_le_bytes());
        for (k, v) in [("TITLE", "七里香"), ("ARTIST", "周杰伦")] {
            let s = format!("{}={}", k, v);
            comment.extend_from_slice(&(s.len() as u32).to_le_bytes());
            comment.extend_from_slice(s.as_bytes());
        }

        let mut buf = page(0, &[ident]);
        buf.extend_from_slice(&page(1, &[comment]));
        let tags = read_tags(&buf);
        assert_eq!(tags.title.as_deref(), Some("七里香"));
        assert_eq!(tags.artist.as_deref(), Some("周杰伦"));
    }

    #[test]
    fn detect_audio_formats() {
        assert_eq!(detect_audio(b"fLaC...."), Some("flac"));
        assert_eq!(detect_audio(b"OggS...."), Some("ogg"));
        assert_eq!(detect_audio(b"ID3\x03\x00"), Some("mp3"));
        assert_eq!(detect_audio(&[0xff, 0xfb, 0x90, 0x00]), Some("mp3"));
        assert_eq!(detect_audio(b"RIFF....WAVE"), Some("wav"));
        assert_eq!(detect_audio(b"junkjunk"), None);
    }

    fn base64_encode(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            out.push(T[(b[0] >> 2) as usize] as char);
            out.push(T[(((b[0] & 3) << 4) | (b[1] >> 4)) as usize] as char);
            out.push(if chunk.len() > 1 {
                T[(((b[1] & 0xf) << 2) | (b[2] >> 6)) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                T[(b[2] & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        out
    }
}
