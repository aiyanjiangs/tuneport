//! 酷我 .kwm 的端到端测试

use tuneport_core::engine::{self, ConvertOptions, Status};
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-kwm");
    let dir = base.join(format!("{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fake_mp3(title: &str, artist: &str) -> Vec<u8> {
    // ID3v2.3 + 两个 UTF-16 文本帧
    let mut frames = Vec::new();
    for (id, text) in [(&b"TIT2"[..], title), (&b"TPE1"[..], artist)] {
        let mut body = vec![1u8, 0xff, 0xfe];
        for u in text.encode_utf16() {
            body.extend_from_slice(&u.to_le_bytes());
        }
        frames.extend_from_slice(id);
        frames.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frames.extend_from_slice(&[0, 0]);
        frames.extend_from_slice(&body);
    }
    let size = frames.len();
    let mut buf = b"ID3\x03\x00\x00".to_vec();
    buf.extend_from_slice(&[
        ((size >> 21) & 0x7f) as u8,
        ((size >> 14) & 0x7f) as u8,
        ((size >> 7) & 0x7f) as u8,
        (size & 0x7f) as u8,
    ]);
    buf.extend_from_slice(&frames);
    buf.extend((0..4000).map(|i| (i * 7 % 251) as u8));
    buf
}

/// 造 kwm:yeelion 头 1024 + (静音 + 音频) 用 32 字节密钥异或
fn build_kwm(plain: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let key: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(11).wrapping_add(5)).collect();
    let mut audio = vec![0u8; 128]; // 前置静音,真实文件里就是这样露的破绽
    audio.extend_from_slice(plain);
    let mut out = vec![0u8; 1024];
    out[..12].copy_from_slice(b"yeelion-kuwo");
    for (i, b) in audio.iter().enumerate() {
        out.push(b ^ key[i & 31]);
    }
    (out, audio)
}

#[test]
fn kwm_end_to_end() {
    let dir = tmpdir("kwm");
    let plain = fake_mp3("富士山下", "陈奕迅");
    let (file, audio) = build_kwm(&plain);

    let src = dir.join("track.kwm");
    fs::write(&src, &file).unwrap();

    let out_dir = dir.join("out");
    let opts = ConvertOptions {
        out_dir: Some(out_dir.clone()),
        ..Default::default()
    };
    let r = engine::convert_one(&src, &opts);

    assert_eq!(r.status, Status::Done, "{:?} {:?}", r.error_code, r.error_msg);
    assert_eq!(r.format.as_deref(), Some("mp3"));
    assert_eq!(r.title, "富士山下");
    assert_eq!(r.artist, "陈奕迅");

    let out = Path::new(r.out.as_deref().unwrap());
    assert_eq!(fs::read(out).unwrap(), audio, "解密后的字节要和原始音频一致");
    assert_eq!(
        out.file_name().unwrap().to_string_lossy(),
        "陈奕迅 - 富士山下.mp3"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn kwm_without_magic_is_rejected() {
    let dir = tmpdir("bad");
    let src = dir.join("fake.kwm");
    fs::write(&src, vec![0u8; 4096]).unwrap();
    let r = engine::convert_one(&src, &ConvertOptions::default());
    assert_eq!(r.status, Status::Failed);
    assert!(r.error_msg.as_deref().unwrap_or("").contains("kwm"), "{:?}", r.error_msg);
    let _ = fs::remove_dir_all(&dir);
}
