//! 酷我音乐 `.kwm` 解密。
//!
//! 结构:`"yeelion-kuwo"` 开头的 1024 字节头 + 音频体;
//! 音频体用一把 **32 字节循环密钥** 异或。
//!
//! 麻烦的地方在于**密钥不写在文件里**:酷我加密时会留下大段静音(全 0)，
//! 密文里就是密钥原样重复，所以可以从"连续两个相同的 32 字节块"里反推出密钥。
//! 找不到就退化成逐个块试解 + 看解出来像不像音频。

/// 头部长度
const HEADER_LEN: usize = 1024;
/// 密钥长度
const KEY_LEN: usize = 32;

pub fn is_kwm_ext(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("kwm")
}

/// 解密结果:音频字节 + 探测到的容器(flac/mp3/ogg/m4a/wav)
pub struct Decrypted {
    pub payload: Vec<u8>,
    pub container: String,
}

/// 判断解密后的数据像不像某种音频
fn sniff(buf: &[u8]) -> Option<&'static str> {
    // 真实文件常有前导静音,先跳过开头的 0 字节
    let start = buf.iter().position(|b| *b != 0).unwrap_or(buf.len());
    let s = &buf[start..];
    if s.len() >= 4 && &s[..4] == b"fLaC" {
        return Some("flac");
    }
    if s.len() >= 4 && &s[..4] == b"OggS" {
        return Some("ogg");
    }
    if s.len() >= 3 && &s[..3] == b"ID3" {
        return Some("mp3");
    }
    if s.len() >= 2 && s[0] == 0xff && (s[1] & 0xe0) == 0xe0 {
        return Some("mp3");
    }
    if s.len() >= 12 && &s[..4] == b"RIFF" && &s[8..12] == b"WAVE" {
        return Some("wav");
    }
    // m4a / mp4:第 4..8 字节是 "ftyp"
    if s.len() >= 12 && &s[4..8] == b"ftyp" {
        return Some("m4a");
    }
    None
}

fn xor_into(body: &[u8], key: &[u8], out: &mut [u8]) {
    for (i, (o, b)) in out.iter_mut().zip(body.iter()).enumerate() {
        *o = b ^ key[i & (KEY_LEN - 1)];
    }
}

/// 用候选密钥试解开头一小段,看能不能认出容器
fn try_key(body: &[u8], key: &[u8]) -> Option<&'static str> {
    let probe_len = 4096.min(body.len());
    if probe_len < 16 {
        return None;
    }
    let mut probe = vec![0u8; probe_len];
    xor_into(&body[..probe_len], key, &mut probe);
    sniff(&probe)
}

/// 从密文里恢复那把 32 字节密钥
fn recover_key(body: &[u8]) -> Option<[u8; KEY_LEN]> {
    let blocks = body.len() / KEY_LEN;
    if blocks == 0 {
        return None;
    }

    // 快路径:连续两个相同的块 = 明文那一段是全 0,块内容就是密钥
    let scan = blocks.min(2048);
    let mut prev = &body[0..KEY_LEN];
    for i in 1..scan {
        let chunk = &body[i * KEY_LEN..(i + 1) * KEY_LEN];
        if chunk == prev {
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(chunk);
            if try_key(body, &key).is_some() {
                return Some(key);
            }
        }
        prev = chunk;
    }

    // 慢路径:把前 64 块依次当密钥试(密钥周期是 32,必然有一个对得上)
    for i in 0..blocks.min(64) {
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&body[i * KEY_LEN..(i + 1) * KEY_LEN]);
        if try_key(body, &key).is_some() {
            return Some(key);
        }
    }
    None
}

/// 解密整个文件
pub fn decrypt(data: &[u8]) -> Result<Decrypted, String> {
    if data.len() < HEADER_LEN + KEY_LEN * 2 {
        return Err("文件过短,不是完整的 kwm".into());
    }
    // 头部不参与解密,只做个校验
    if !data.starts_with(b"yeelion-kuwo") && !data.starts_with(b"yeelion") {
        return Err("不是 kwm 文件(缺少 yeelion 标识)".into());
    }

    let body = &data[HEADER_LEN..];
    let key = recover_key(body).ok_or("无法从文件里恢复出密钥(可能不是标准 kwm)")?;

    let mut payload = vec![0u8; body.len()];
    xor_into(body, &key, &mut payload);

    let container = sniff(&payload).unwrap_or("mp3").to_string();
    Ok(Decrypted { payload, container })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 kwm:头部 + 前置静音 + 真实音频,用给定密钥加密
    fn build_kwm(plain: &[u8], key: &[u8; KEY_LEN], lead_silence: usize) -> Vec<u8> {
        let mut audio = vec![0u8; lead_silence];
        audio.extend_from_slice(plain);
        let mut out = vec![0u8; HEADER_LEN];
        out[..12].copy_from_slice(b"yeelion-kuwo");
        for (i, b) in audio.iter().enumerate() {
            out.push(b ^ key[i & (KEY_LEN - 1)]);
        }
        out
    }

    fn fake_mp3() -> Vec<u8> {
        let mut v = vec![0xff, 0xfb, 0x90, 0x00];
        v.extend((0..3000).map(|i| (i * 13 % 251) as u8));
        v
    }

    #[test]
    fn recovers_key_from_silence() {
        let key: [u8; KEY_LEN] = core::array::from_fn(|i| (i as u8).wrapping_mul(7).wrapping_add(3));
        let plain = fake_mp3();
        let file = build_kwm(&plain, &key, 256); // 前置静音 → 密文里出现连续相同块
        let d = decrypt(&file).unwrap();
        assert_eq!(d.container, "mp3");
        // 静音 + 原文都要还原
        assert_eq!(d.payload.len(), 256 + plain.len());
        assert_eq!(&d.payload[256..], &plain[..]);
    }

    #[test]
    fn finds_silence_inside_the_audio() {
        // 静音不一定在最前面:放在中间也要能找到
        let key: [u8; KEY_LEN] = core::array::from_fn(|i| (i as u8) ^ 0x5a);
        let mut plain = fake_mp3();
        plain.extend(std::iter::repeat(0u8).take(256)); // 中段静音
        plain.extend((0..1000).map(|i| (i % 53) as u8));
        let file = build_kwm(&plain, &key, 0);
        let d = decrypt(&file).unwrap();
        assert_eq!(&d.payload, &plain);
    }

    #[test]
    fn rejects_non_kwm() {
        let data = vec![0u8; 2000];
        assert!(decrypt(&data).is_err());
    }

    #[test]
    fn detects_flac_container() {
        let key = [9u8; KEY_LEN];
        let mut plain = b"fLaC".to_vec();
        plain.extend((0..2000).map(|i| (i % 97) as u8));
        let file = build_kwm(&plain, &key, 64);
        let d = decrypt(&file).unwrap();
        assert_eq!(d.container, "flac");
    }
}
