//! 迁移闸门:把 JS 版验证过的断言在 Rust 上重跑一遍。

mod fixture;

use std::io::Cursor;

use tuneport_core::ncm::{decode_audio, inspect, AudioFormat, Ncm, NcmError};
use tuneport_core::tags;

fn decode(bytes: &[u8]) -> Result<(Ncm, Vec<u8>), NcmError> {
    let mut cur = Cursor::new(bytes.to_vec());
    let ncm = inspect(&mut cur, bytes.len() as u64)?;
    let start = ncm.audio_start;
    let mut out = Vec::new();
    decode_audio(&mut cur, start, ncm.audio_end, &ncm.key_stream, start, |b| {
        out.extend_from_slice(b);
        Ok(())
    })?;
    Ok((ncm, out))
}

#[test]
fn three_layouts_locate_audio() {
    let payload: Vec<u8> = (0..30000u32).map(|i| (i % 251) as u8).collect();
    let flac = fixture::fake_flac(&payload);

    for layout in ['A', 'B', 'C'] {
        let s = fixture::make_sample(layout, "flac", &flac, "夏日信笺", "林墨");
        let (ncm, audio) = decode(&s.bytes).expect("应当解析成功");
        assert_eq!(ncm.layout, layout, "布局识别错误");
        assert_eq!(ncm.audio_start as usize, s.expected_start, "音频起点不一致");
        assert_eq!(audio, s.audio, "解密结果与原文不一致");
        assert_eq!(ncm.format, AudioFormat::Flac);
    }
}

#[test]
fn meta_is_parsed() {
    let s = fixture::make_sample('A', "flac", &fixture::fake_flac(&[1, 2, 3]), "夏日信笺", "林墨");
    let (ncm, _) = decode(&s.bytes).unwrap();
    let meta = ncm.meta.expect("应当有元信息");
    assert_eq!(meta.title, "夏日信笺");
    assert_eq!(meta.artist, "林墨");
    assert_eq!(meta.album, "测试专辑");
    assert_eq!(meta.format, "flac");
    assert!(!ncm.meta_missing);
}

#[test]
fn mp3_is_detected_and_decoded() {
    let payload: Vec<u8> = (0..20000u32).map(|i| (i % 239) as u8).collect();
    let mp3 = fixture::fake_mp3(&payload);
    let s = fixture::make_sample('B', "mp3", &mp3, "北方的雨", "陈屿");
    let (ncm, audio) = decode(&s.bytes).unwrap();
    assert_eq!(ncm.format, AudioFormat::Mp3);
    assert_eq!(audio, s.audio);
}

#[test]
fn rejects_non_ncm() {
    let mut bytes = b"THIS IS NOT AN NCM FILE AT ALL, JUST PLAIN TEXT.".to_vec();
    bytes.resize(64, 0);
    let mut cur = Cursor::new(bytes.clone());
    let err = inspect(&mut cur, bytes.len() as u64).unwrap_err();
    assert!(matches!(err, NcmError::Format), "应报 Format,实际:{:?}", err);
}

#[test]
fn rejects_truncated() {
    let s = fixture::make_sample('A', "flac", &fixture::fake_flac(&[9; 512]), "t", "a");
    let truncated = &s.bytes[..40];
    let mut cur = Cursor::new(truncated.to_vec());
    assert!(inspect(&mut cur, truncated.len() as u64).is_err());
}

#[test]
fn id3_tag_is_parseable() {
    let t = tags::Tags {
        title: "夏日信笺".into(),
        artist: "林墨".into(),
        album: "白日梦游".into(),
        ..Default::default()
    };
    let tag = tags::build_id3v23(&t);
    assert_eq!(&tag[0..3], b"ID3");
    assert_eq!(tags::id3v2_len(&tag), tag.len(), "整体长度与头部声明一致");
}

#[test]
fn flac_header_rewrite_preserves_frames() {
    let frames: Vec<u8> = (0..4096u32).map(|i| (i % 97) as u8).collect();
    let flac = fixture::fake_flac(&frames);
    let (blocks, end) = tags::read_flac_blocks(&flac).unwrap();
    let t = tags::Tags {
        title: "T".into(),
        artist: "A".into(),
        album: "B".into(),
        ..Default::default()
    };
    let header = tags::build_flac_header(&blocks, &t, 1024).unwrap();
    let rebuilt: Vec<u8> = header.into_iter().chain(flac[end..].iter().copied()).collect();
    let (_, end2) = tags::read_flac_blocks(&rebuilt).unwrap();
    assert_eq!(&rebuilt[end2..], &frames[..], "音频帧被改动");
}
