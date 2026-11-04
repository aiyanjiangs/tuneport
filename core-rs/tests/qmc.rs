//! QQ 音乐格式的端到端测试:造一个"加密"文件 → 转换 → 比对字节。

use tuneport_core::engine::{self, ConvertOptions, Status};
use tuneport_core::qmc;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-qmc");
    let dir = base.join(format!("{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 造一段带 FLAC 标签的假音频(足够被识别为 flac,并能读出标签)
fn fake_flac(title: &str, artist: &str) -> Vec<u8> {
    let mut vc = Vec::new();
    vc.extend_from_slice(&0u32.to_le_bytes()); // vendor
    vc.extend_from_slice(&2u32.to_le_bytes());
    for (k, v) in [("TITLE", title), ("ARTIST", artist)] {
        let s = format!("{}={}", k, v);
        vc.extend_from_slice(&(s.len() as u32).to_le_bytes());
        vc.extend_from_slice(s.as_bytes());
    }
    let mut buf = b"fLaC".to_vec();
    buf.push(0x80 | 4); // 最后一个块,类型 VORBIS_COMMENT
    let len = vc.len() as u32;
    buf.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
    buf.extend_from_slice(&vc);
    // 假的音频负载
    for i in 0..4096u32 {
        buf.push((i * 31 % 253) as u8);
    }
    buf
}

#[test]
fn qmc1_end_to_end() {
    let dir = tmpdir("qmc1");
    let plain = fake_flac("晴天", "周杰伦");

    // 用同一把异或表"加密"
    let mut enc = plain.clone();
    qmc::Cipher::static_cipher().decrypt(0, &mut enc);

    let src = dir.join("测试.qmcflac");
    fs::write(&src, &enc).unwrap();
    // 同名歌词
    fs::write(dir.join("测试.lrc"), "[00:00]歌词").unwrap();

    let out_dir = dir.join("out");
    let opts = ConvertOptions {
        out_dir: Some(out_dir.clone()),
        copy_lrc: true,
        ..Default::default()
    };
    let r = engine::convert_one(&src, &opts);

    assert_eq!(r.status, Status::Done, "{:?}", r.error_msg);
    assert_eq!(r.format.as_deref(), Some("flac"));
    assert_eq!(r.title, "晴天");
    assert_eq!(r.artist, "周杰伦");

    let out = Path::new(r.out.as_deref().unwrap());
    assert!(out.exists(), "输出文件不存在");
    assert_eq!(fs::read(out).unwrap(), plain, "解出来的字节要和原始音频一致");
    assert!(r.lrc_copied, "歌词应该一起复制");
    assert_eq!(
        out.file_name().unwrap().to_string_lossy(),
        "周杰伦 - 晴天.flac"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn qmc2_v1_footer_end_to_end() {
    let dir = tmpdir("qmc2");
    let plain = fake_flac("夜曲", "周杰伦");

    // 造一把能被 parse_ekey 原样接受的关键:
    // 8 字节头 + 非 8 倍数的主体 => TC-TEA 解不开 => 当作裸密钥
    let mut raw_key = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9];
    raw_key.extend_from_slice(&[10u8, 11, 12]);
    let ekey = qmc::base64_encode(&raw_key);
    let cipher = qmc::Cipher::from_ekey(&ekey).unwrap();

    let mut enc = plain.clone();
    cipher.decrypt(0, &mut enc);

    // .mflac = 加密音频 + 原始 ekey 字节 + 4 字节长度(V1 尾部)
    let mut file = enc.clone();
    file.extend_from_slice(&raw_key);
    file.extend_from_slice(&(raw_key.len() as u32).to_le_bytes());

    let src = dir.join("sample.mflac");
    fs::write(&src, &file).unwrap();

    let opts = ConvertOptions::default(); // 输出到源目录
    let r = engine::convert_one(&src, &opts);

    assert_eq!(r.status, Status::Done, "{:?}", r.error_msg);
    assert_eq!(r.format.as_deref(), Some("flac"));
    let out = Path::new(r.out.as_deref().unwrap());
    assert_eq!(fs::read(out).unwrap(), plain, "QMC2 解出来要和原文一致");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn musicex_reports_clear_error() {
    let dir = tmpdir("musicex");
    // 新版尾部:只有元数据,没有密钥
    let mut data = vec![0u8; 200];
    let footer_size = 16u32;
    data.extend_from_slice(&footer_size.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(b"musicex\0");

    let src = dir.join("new.mflac");
    fs::write(&src, &data).unwrap();
    let r = engine::convert_one(&src, &ConvertOptions::default());

    assert_eq!(r.status, Status::Failed);
    assert_eq!(r.error_code.as_deref(), Some("E_EKEY"));
    assert!(
        r.error_msg.as_deref().unwrap_or("").contains("联网"),
        "错误信息要说明需要联网: {:?}",
        r.error_msg
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scan_folder_picks_up_both_families() {
    let dir = tmpdir("scan");
    for name in ["a.ncm", "b.qmcflac", "c.mflac", "d.mgg", "e.mp3", "f.txt"] {
        fs::write(dir.join(name), b"x").unwrap();
    }
    let mut out = Vec::new();
    engine::scan_folder(&dir, &mut out);
    let names: Vec<String> = out
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names.len(), 4, "应该只收 ncm + qmc 系列: {:?}", names);
    assert!(names.contains(&"a.ncm".to_string()));
    assert!(names.contains(&"b.qmcflac".to_string()));
    assert!(names.contains(&"c.mflac".to_string()));
    assert!(names.contains(&"d.mgg".to_string()));

    let _ = fs::remove_dir_all(&dir);
}
