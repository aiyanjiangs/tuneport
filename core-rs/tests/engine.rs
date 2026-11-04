//! 引擎端到端:造 ncm → 真写盘 → 校验产物(标签/音频帧/歌词/重名)

mod fixture;

use std::fs;
use std::path::{Path, PathBuf};

use tuneport_core::engine::{convert_one, run_batch, ConvertOptions, Overwrite, Status};
use tuneport_core::tags;

fn tmp_root(tag: &str) -> PathBuf {
    // 用 target/ 下的目录而不是系统 TEMP:某些环境对临时目录写入有限制,
    // 而 target/ 一定在工程内、可写。
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp");
    let p = base.join(format!("{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn write_src(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn flac_end_to_end() {
    let root = tmp_root("flac");
    let src_dir = root.join("src");
    let out_dir = root.join("out");
    fs::create_dir_all(&src_dir).unwrap();

    let payload: Vec<u8> = (0..20000u32).map(|i| (i % 251) as u8).collect();
    let flac = fixture::fake_flac(&payload);
    let s = fixture::make_sample('A', "flac", &flac, "夏日信笺", "林墨");
    let src = write_src(&src_dir, "a.ncm", &s.bytes);
    fs::write(src_dir.join("a.lrc"), "[00:01.00]测试歌词\n").unwrap();

    let opts = ConvertOptions {
        out_dir: Some(out_dir.clone()),
        copy_lrc: true,
        ..Default::default()
    };
    let r = convert_one(&src, &opts);
    assert_eq!(r.status, Status::Done, "{:?} {:?}", r.error_code, r.error_msg);

    let out = PathBuf::from(r.out.clone().unwrap());
    assert_eq!(out.file_name().unwrap().to_string_lossy(), "林墨 - 夏日信笺.flac");
    assert!(out.exists());

    let buf = fs::read(&out).unwrap();
    assert_eq!(&buf[0..4], b"fLaC");
    let (blocks, end) = tags::read_flac_blocks(&buf).unwrap();
    let vc = blocks.iter().find(|b| b.kind == 4).expect("应有 VORBIS_COMMENT");
    let text = String::from_utf8_lossy(&vc.data);
    assert!(text.contains("TITLE=夏日信笺"), "{}", text);
    assert!(text.contains("ARTIST=林墨"), "{}", text);
    assert!(text.contains("ALBUM=测试专辑"), "{}", text);
    assert_eq!(&buf[end..], &flac[42..], "音频帧应原样保留");

    assert!(out_dir.join("林墨 - 夏日信笺.lrc").exists(), "歌词应被复制");
    assert!(r.lrc_copied);
    assert!(r.has_lrc);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn mp3_end_to_end_and_dedup() {
    let root = tmp_root("mp3");
    let src_dir = root.join("src");
    let out_dir = root.join("out");
    fs::create_dir_all(&src_dir).unwrap();

    let payload: Vec<u8> = (0..12000u32).map(|i| (i % 239) as u8).collect();
    let mp3 = fixture::fake_mp3(&payload);
    let s = fixture::make_sample('B', "mp3", &mp3, "北方的雨", "陈屿");
    let src = write_src(&src_dir, "b.ncm", &s.bytes);

    let opts = ConvertOptions { out_dir: Some(out_dir.clone()), copy_lrc: false, ..Default::default() };

    let r1 = convert_one(&src, &opts);
    assert_eq!(r1.status, Status::Done);
    let out1 = PathBuf::from(r1.out.unwrap());
    assert_eq!(out1.file_name().unwrap().to_string_lossy(), "陈屿 - 北方的雨.mp3");

    let buf = fs::read(&out1).unwrap();
    assert_eq!(&buf[0..3], b"ID3");
    let tag_len = tags::id3v2_len(&buf);
    assert_eq!(&buf[tag_len..], &mp3[..], "MP3 音频应原样保留");

    // 再转一次 → 自动加 (2)
    let r2 = convert_one(&src, &opts);
    let out2 = PathBuf::from(r2.out.unwrap());
    assert_eq!(out2.file_name().unwrap().to_string_lossy(), "陈屿 - 北方的雨 (2).mp3");

    // overwrite=skip → 跳过
    let skip_opts = ConvertOptions { out_dir: Some(out_dir.clone()), overwrite: Overwrite::Skip, ..Default::default() };
    let r3 = convert_one(&src, &skip_opts);
    assert_eq!(r3.status, Status::Skipped);
    let skipped_out = r3.out.clone().unwrap_or_default();
    assert!(Path::new(&skipped_out).exists(), "跳过的文件也应返回已存在的路径:{:?}", r3.out);
    assert_eq!(r3.format.as_deref(), Some("mp3"), "跳过的文件也要带上格式");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn batch_reports_each_file() {
    let root = tmp_root("batch");
    let src_dir = root.join("src");
    let out_dir = root.join("out");
    fs::create_dir_all(&src_dir).unwrap();

    let mut files = Vec::new();
    for i in 0..3 {
        let s = fixture::make_sample('A', "flac", &fixture::fake_flac(&[i as u8; 2048]), &format!("歌{}", i), "歌手");
        files.push(write_src(&src_dir, &format!("{}.ncm", i), &s.bytes));
    }
    // 一个坏文件
    let bad = write_src(&src_dir, "bad.ncm", b"THIS IS NOT AN NCM FILE AT ALL, JUST PLAIN TEXT....");
    files.push(bad);

    let opts = ConvertOptions { out_dir: Some(out_dir.clone()), copy_lrc: false, ..Default::default() };
    let mut done = 0;
    let mut failed = 0;
    run_batch(&files, &opts, 3, None, |_| {}, |r| match r.status {
        Status::Done => done += 1,
        Status::Failed => failed += 1,
        _ => {}
    });

    assert_eq!(done, 3, "3 个应成功");
    assert_eq!(failed, 1, "坏文件应失败且不影响其他");
    assert_eq!(fs::read_dir(&out_dir).unwrap().count(), 3);

    let _ = fs::remove_dir_all(&root);
}
