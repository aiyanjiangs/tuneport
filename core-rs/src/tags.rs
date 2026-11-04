//! 标签写入:MP3 → ID3v2.3(UTF-16),FLAC → Vorbis Comment(UTF-8)。
//! 手写字节,不依赖标签库。

use std::fmt;

#[derive(Debug)]
pub enum TagError {
    Flac(String),
    Encode(String),
}
impl fmt::Display for TagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TagError::Flac(s) => write!(f, "FLAC 结构异常:{}", s),
            TagError::Encode(s) => write!(f, "编码失败:{}", s),
        }
    }
}
impl std::error::Error for TagError {}

#[derive(Debug, Clone, Default)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track: String,
    pub year: String,
}

fn synchsafe(n: usize) -> [u8; 4] {
    [
        ((n >> 21) & 0x7f) as u8,
        ((n >> 14) & 0x7f) as u8,
        ((n >> 7) & 0x7f) as u8,
        (n & 0x7f) as u8,
    ]
}

pub fn read_synchsafe(b: &[u8], off: usize) -> usize {
    ((b[off] & 0x7f) as usize) << 21
        | ((b[off + 1] & 0x7f) as usize) << 14
        | ((b[off + 2] & 0x7f) as usize) << 7
        | (b[off + 3] & 0x7f) as usize
}

fn text_frame(id: &[u8; 4], text: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(0x01); // UTF-16 with BOM
    body.extend_from_slice(&[0xff, 0xfe]);
    for u in text.encode_utf16() {
        body.extend_from_slice(&u.to_le_bytes());
    }
    body.extend_from_slice(&[0x00, 0x00]);

    let mut out = Vec::with_capacity(10 + body.len());
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes()); // v2.3 帧长度是普通大端
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&body);
    out
}

/// 生成 ID3v2.3 标签块(10 字节头 + 若干帧)
pub fn build_id3v23(t: &Tags) -> Vec<u8> {
    let mut frames = Vec::new();
    if !t.title.is_empty() {
        frames.extend(text_frame(b"TIT2", &t.title));
    }
    if !t.artist.is_empty() {
        frames.extend(text_frame(b"TPE1", &t.artist));
    }
    if !t.album.is_empty() {
        frames.extend(text_frame(b"TALB", &t.album));
    }
    if !t.track.is_empty() {
        frames.extend(text_frame(b"TRCK", &t.track));
    }
    if !t.year.is_empty() {
        frames.extend(text_frame(b"TYER", &t.year));
    }
    let mut out = Vec::with_capacity(10 + frames.len());
    out.extend_from_slice(b"ID3");
    out.extend_from_slice(&[0x03, 0x00, 0x00]);
    out.extend_from_slice(&synchsafe(frames.len()));
    out.extend_from_slice(&frames);
    out
}

/// 若开头已有 ID3v2,返回需要跳过的字节数
pub fn id3v2_len(head: &[u8]) -> usize {
    if head.len() >= 10 && &head[0..3] == b"ID3" {
        let size = read_synchsafe(head, 6);
        let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
        return 10 + size + footer;
    }
    0
}

#[derive(Debug, Clone)]
pub struct FlacBlock {
    pub kind: u8,
    pub data: Vec<u8>,
}

/// 解析 "fLaC" 之后的元数据块
pub fn read_flac_blocks(buf: &[u8]) -> Result<(Vec<FlacBlock>, usize), TagError> {
    if buf.len() < 8 || &buf[0..4] != b"fLaC" {
        return Err(TagError::Flac("魔数不匹配".into()));
    }
    let mut blocks = Vec::new();
    let mut pos = 4usize;
    for _ in 0..128 {
        if pos + 4 > buf.len() {
            return Err(TagError::Flac("元数据块被截断".into()));
        }
        let h = buf[pos];
        let kind = h & 0x7f;
        let last = h & 0x80 != 0;
        let len = ((buf[pos + 1] as usize) << 16) | ((buf[pos + 2] as usize) << 8) | buf[pos + 3] as usize;
        if pos + 4 + len > buf.len() {
            return Err(TagError::Flac("元数据块长度越界".into()));
        }
        blocks.push(FlacBlock {
            kind,
            data: buf[pos + 4..pos + 4 + len].to_vec(),
        });
        pos += 4 + len;
        if last {
            return Ok((blocks, pos));
        }
    }
    Err(TagError::Flac("元数据块过多".into()))
}

fn vorbis_body(t: &Tags) -> Vec<u8> {
    let vendor = b"TunePort 1.0";
    let mut comments: Vec<String> = Vec::new();
    if !t.title.is_empty() {
        comments.push(format!("TITLE={}", t.title));
    }
    if !t.artist.is_empty() {
        comments.push(format!("ARTIST={}", t.artist));
    }
    if !t.album.is_empty() {
        comments.push(format!("ALBUM={}", t.album));
    }
    if !t.track.is_empty() {
        comments.push(format!("TRACKNUMBER={}", t.track));
    }
    if !t.year.is_empty() {
        comments.push(format!("DATE={}", t.year));
    }

    let mut out = Vec::new();
    out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    out.extend_from_slice(vendor);
    out.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for c in comments {
        let b = c.as_bytes();
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }
    out
}

/// 重建 FLAC 头部:保留 STREAMINFO/其他块,替换 VORBIS_COMMENT,补 PADDING
pub fn build_flac_header(blocks: &[FlacBlock], t: &Tags, padding: usize) -> Result<Vec<u8>, TagError> {
    if blocks.is_empty() || blocks[0].kind != 0 {
        return Err(TagError::Flac("缺少 STREAMINFO".into()));
    }
    let mut out: Vec<FlacBlock> = Vec::new();
    out.push(FlacBlock { kind: 0, data: blocks[0].data.clone() });
    out.push(FlacBlock { kind: 4, data: vorbis_body(t) });
    for b in blocks.iter().skip(1) {
        if b.kind == 4 || b.kind == 1 {
            continue; // 丢掉旧的注释块与填充
        }
        out.push(b.clone());
    }
    if padding > 0 {
        out.push(FlacBlock { kind: 1, data: vec![0u8; padding] });
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"fLaC");
    let n = out.len();
    for (i, b) in out.iter().enumerate() {
        let last = if i == n - 1 { 0x80 } else { 0x00 };
        let len = b.data.len();
        bytes.push(last | (b.kind & 0x7f));
        bytes.push(((len >> 16) & 0xff) as u8);
        bytes.push(((len >> 8) & 0xff) as u8);
        bytes.push((len & 0xff) as u8);
        bytes.extend_from_slice(&b.data);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id3_frames_roundtrip() {
        let t = Tags {
            title: "夏日信笺".into(),
            artist: "林墨 / 苏晚".into(),
            album: "白日梦游".into(),
            ..Default::default()
        };
        let buf = build_id3v23(&t);
        assert_eq!(&buf[0..3], b"ID3");
        assert_eq!(buf[3], 3);
        let size = read_synchsafe(&buf, 6);
        assert_eq!(buf.len(), size + 10);

        let mut p = 10;
        let mut found = std::collections::HashMap::new();
        while p + 10 <= buf.len() {
            let id = String::from_utf8_lossy(&buf[p..p + 4]).to_string();
            if !id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
                break;
            }
            let len = u32::from_be_bytes([buf[p + 4], buf[p + 5], buf[p + 6], buf[p + 7]]) as usize;
            let body = &buf[p + 10..p + 10 + len];
            assert_eq!(body[0], 0x01);
            let mut units = Vec::new();
            let mut i = 3; // 跳过编码字节 + BOM
            while i + 1 < body.len() {
                let u = u16::from_le_bytes([body[i], body[i + 1]]);
                if u == 0 {
                    break;
                }
                units.push(u);
                i += 2;
            }
            found.insert(id, String::from_utf16(&units).unwrap());
            p += 10 + len;
        }
        assert_eq!(found.get("TIT2").map(|s| s.as_str()), Some("夏日信笺"));
        assert_eq!(found.get("TPE1").map(|s| s.as_str()), Some("林墨 / 苏晚"));
        assert_eq!(found.get("TALB").map(|s| s.as_str()), Some("白日梦游"));
    }

    #[test]
    fn flac_header_rebuild_keeps_audio() {
        let mut streaminfo = vec![0u8; 34];
        streaminfo[0] = 0x10;
        let mut src = Vec::new();
        src.extend_from_slice(b"fLaC");
        src.extend_from_slice(&[0x80, 0x00, 0x00, 34]);
        src.extend_from_slice(&streaminfo);
        let frames = vec![0xAAu8; 2048];
        src.extend_from_slice(&frames);

        let (blocks, end) = read_flac_blocks(&src).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(end, 4 + 4 + 34);

        let t = Tags { title: "T".into(), artist: "A".into(), album: "B".into(), ..Default::default() };
        let header = build_flac_header(&blocks, &t, 1024).unwrap();
        let (re, end2) = read_flac_blocks(&header).unwrap();
        assert_eq!(re[0].kind, 0);
        assert!(re.iter().any(|b| b.kind == 4));
        assert_eq!(re.last().unwrap().kind, 1);
        let text = String::from_utf8_lossy(&re.iter().find(|b| b.kind == 4).unwrap().data).to_string();
        assert!(text.contains("TITLE=T"));
        assert!(text.contains("ARTIST=A"));
        assert!(text.contains("ALBUM=B"));

        let rebuilt: Vec<u8> = header.into_iter().chain(src[end..].iter().copied()).collect();
        let (_, end3) = read_flac_blocks(&rebuilt).unwrap();
        assert_eq!(end3, end2);
        assert_eq!(&rebuilt[end3..], &frames[..]);
    }
}
