//! 按 ncm 规范构造测试样本(仅测试用)

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;

use tuneport_core::ncm::{build_key_stream, CORE_KEY, META_KEY};

pub fn aes_encrypt(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let pad = 16 - (data.len() % 16);
    let mut buf = data.to_vec();
    buf.extend(std::iter::repeat(pad as u8).take(pad));
    let cipher = Aes128::new(GenericArray::from_slice(key));
    for chunk in buf.chunks_exact_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(chunk));
    }
    buf
}

pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

pub struct Sample {
    pub bytes: Vec<u8>,
    pub audio: Vec<u8>,
    pub expected_start: usize,
}

pub fn make_sample(layout: char, format: &str, audio: &[u8], title: &str, artist: &str) -> Sample {
    // 固定的 key 段(17 字节前缀 + 16 字节随机体,这里用固定模式保证可复现)
    let mut key_plain = b"neteasecloudmusic".to_vec();
    key_plain.extend((0u8..16).map(|i| i.wrapping_mul(7).wrapping_add(3)));

    let mut key_cipher = aes_encrypt(&CORE_KEY, &key_plain);
    for b in key_cipher.iter_mut() {
        *b ^= 0x64;
    }

    let json = format!(
        "{{\"musicName\":\"{}\",\"artist\":[[\"{}\",1]],\"album\":\"测试专辑\",\"format\":\"{}\"}}",
        title, artist, format
    );
    let meta_cipher = aes_encrypt(&META_KEY, format!("music:{}", json).as_bytes());
    let mut meta_section = b"163 key(Don't modify):".to_vec();
    meta_section.extend_from_slice(base64_encode(&meta_cipher).as_bytes());
    for b in meta_section.iter_mut() {
        *b ^= 0x63;
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"CTENFDAM");
    bytes.extend_from_slice(&[0x01, 0x70]);
    bytes.extend_from_slice(&(key_cipher.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&key_cipher);
    bytes.extend_from_slice(&(meta_section.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&meta_section);

    let mut image = vec![0u8; 2048];
    image[0] = 0xff;
    image[1] = 0xd8;
    image[2] = 0xff;
    image[3] = 0xe0;
    let ilen = (image.len() as u32).to_le_bytes();

    match layout {
        'A' => {
            bytes.extend_from_slice(&[0u8; 4]); // crc32
            bytes.push(0x01); // gap
            bytes.extend_from_slice(&ilen);
            bytes.extend_from_slice(&image);
        }
        'B' => {
            bytes.extend_from_slice(&[0u8; 4]);
            bytes.extend_from_slice(&[0u8; 5]); // gap
            bytes.extend_from_slice(&ilen);
            bytes.extend_from_slice(&image);
        }
        _ => {
            bytes.extend_from_slice(&[0u8; 4]);
            bytes.push(0x01);
            bytes.extend_from_slice(&ilen);
            bytes.extend_from_slice(&[0u8; 4]); // pad
            bytes.extend_from_slice(&image);
        }
    }

    let expected_start = bytes.len();
    let ks = build_key_stream(&key_plain[17..]);
    let mut enc = audio.to_vec();
    for (i, b) in enc.iter_mut().enumerate() {
        *b ^= ks[i & 0xff];
    }
    bytes.extend_from_slice(&enc);

    Sample {
        bytes,
        audio: audio.to_vec(),
        expected_start,
    }
}

/// 一个结构合法的假 FLAC:STREAMINFO 是唯一块(last 位置位)+ 音频帧
pub fn fake_flac(frames: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"fLaC");
    v.extend_from_slice(&[0x80, 0x00, 0x00, 34]);
    v.extend_from_slice(&[0u8; 34]);
    v.extend_from_slice(frames);
    v
}

pub fn fake_mp3(frames: &[u8]) -> Vec<u8> {
    let mut v = vec![0xff, 0xfb, 0x90, 0x00];
    v.extend_from_slice(frames);
    v
}
