//! 批量转换引擎:扫描 → 解密 → 写标签 → 落盘 → 拷歌词。
//! 全程流式,单文件内存占用约 1 MB。

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use serde::Serialize;

use crate::naming;
use crate::ncm::{self, AudioFormat, Ncm, NcmError};
use crate::tags::{self, TagError, Tags};

const CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overwrite {
    Rename,
    Skip,
    Overwrite,
}

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub out_dir: Option<PathBuf>,
    pub copy_lrc: bool,
    pub template: String,
    pub overwrite: Overwrite,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            out_dir: None,
            copy_lrc: true,
            template: "{artist} - {title}".to_string(),
            overwrite: Overwrite::Rename,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Done,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertResult {
    pub src: String,
    pub status: Status,
    pub out: Option<String>,
    pub format: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub has_lrc: bool,
    pub lrc_copied: bool,
    pub error_code: Option<String>,
    pub error_msg: Option<String>,
    pub ms: u128,
}

impl ConvertResult {
    fn new(src: &Path) -> Self {
        Self {
            src: src.to_string_lossy().to_string(),
            status: Status::Done,
            out: None,
            format: None,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            has_lrc: false,
            lrc_copied: false,
            error_code: None,
            error_msg: None,
            ms: 0,
        }
    }
}

fn io_err(e: std::io::Error) -> NcmError {
    NcmError::Io(e)
}

fn tag_err(e: TagError) -> NcmError {
    NcmError::Corrupt(e.to_string())
}

fn err_code(e: &NcmError) -> (String, String) {
    let code = match e {
        NcmError::Format => "E_FORMAT",
        NcmError::Corrupt(_) => "E_CORRUPT",
        NcmError::Key => "E_KEY",
        NcmError::Meta(_) => "E_META",
        NcmError::NeedOnline => "E_EKEY",
        NcmError::Io(_) => "E_IO",
    };
    (code.to_string(), e.to_string())
}

/// 扩展名是否受支持(ncm + QQ 音乐那一套)
pub fn is_supported(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let e = ext.to_ascii_lowercase();
            e == "ncm" || crate::qmc::is_qmc_ext(&e) || crate::kwm::is_kwm_ext(&e)
        }
        None => false,
    }
}

/// 递归扫描目录下的 .ncm(跳过隐藏目录)
pub fn scan_folder(dir: &Path, out: &mut Vec<PathBuf>) {
    scan_inner(dir, out, 0);
}

fn scan_inner(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 12 {
        return;
    }
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => scan_inner(&path, out, depth + 1),
            Ok(t) if t.is_file() => {
                if is_supported(&path) {
                    out.push(path);
                }
            }
            _ => {}
        }
    }
}

/// 找同名歌词。优先 "<文件名>.lrc",其次 "<歌名>.lrc"(有些下载器用歌名命名)。
fn find_lrc(src: &Path, title: &str) -> Option<PathBuf> {
    let dir = src.parent()?;
    let stem = src.file_stem()?.to_string_lossy().to_lowercase();
    let mut wants: Vec<String> = vec![format!("{}.lrc", stem)];

    let t = title.trim().to_lowercase();
    if !t.is_empty() && t != stem {
        wants.push(format!("{}.lrc", t));
    }
    // "歌手 - 歌名.ncm" → 也试试 "歌名.lrc"
    if let Some((_, after)) = stem.rsplit_once(" - ") {
        let s = format!("{}.lrc", after.trim());
        if !wants.contains(&s) {
            wants.push(s);
        }
    }

    let rd = fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if wants.contains(&name) {
            return Some(e.path());
        }
    }
    None
}

fn read_decrypted(
    file: &mut File,
    start: u64,
    len: usize,
    ks: &[u8; 256],
    origin: u64,
) -> Result<Vec<u8>, NcmError> {
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    let mut got = 0usize;
    while got < len {
        let n = file.read(&mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    buf.truncate(got);
    let base = start.saturating_sub(origin) as usize;
    for (i, b) in buf.iter_mut().enumerate() {
        *b ^= ks[(base + i) & 0xff];
    }
    Ok(buf)
}

fn tags_of(res: &ConvertResult) -> Tags {
    Tags {
        title: res.title.clone(),
        artist: res.artist.clone(),
        album: res.album.clone(),
        ..Default::default()
    }
}

fn write_audio(
    file: &mut File,
    ncm: &Ncm,
    out: &mut impl Write,
    res: &ConvertResult,
) -> Result<(), NcmError> {
    match ncm.format {
        AudioFormat::Mp3 => {
            // 丢掉源文件里可能已有的 ID3v2,换成我们写的
            let head = read_decrypted(file, ncm.audio_start, 10, &ncm.key_stream, ncm.audio_start)?;
            let skip = tags::id3v2_len(&head) as u64;
            let t = tags_of(res);
            if !(t.title.is_empty() && t.artist.is_empty() && t.album.is_empty()) {
                out.write_all(&tags::build_id3v23(&t)).map_err(io_err)?;
            }
            let start = ncm.audio_start + skip;
            ncm::decode_audio(file, start, ncm.audio_end, &ncm.key_stream, ncm.audio_start, |buf| {
                out.write_all(buf).map_err(io_err)
            })?;
        }
        AudioFormat::Flac => {
            let total = ncm.audio_end - ncm.audio_start;
            let mut head_len = std::cmp::min(CHUNK as u64, total) as usize;
            let parsed = loop {
                let plain = read_decrypted(file, ncm.audio_start, head_len, &ncm.key_stream, ncm.audio_start)?;
                match tags::read_flac_blocks(&plain) {
                    Ok(v) => break v,
                    Err(e) => {
                        if head_len as u64 >= total || head_len > (16 << 20) {
                            return Err(tag_err(e));
                        }
                        head_len = std::cmp::min(total as usize, head_len * 4);
                    }
                }
            };
            let (blocks, end) = parsed;
            let header = tags::build_flac_header(&blocks, &tags_of(res), 1024).map_err(tag_err)?;
            out.write_all(&header).map_err(io_err)?;
            ncm::decode_audio(
                file,
                ncm.audio_start + end as u64,
                ncm.audio_end,
                &ncm.key_stream,
                ncm.audio_start,
                |buf| out.write_all(buf).map_err(io_err),
            )?;
        }
    }
    Ok(())
}

fn convert_inner(src: &Path, opts: &ConvertOptions, res: &mut ConvertResult) -> Result<(), NcmError> {
    // 先按扩展名分流:QQ 音乐 / 酷我走各自的流程
    if let Some(ext) = src.extension().and_then(|e| e.to_str()) {
        if crate::qmc::is_qmc_ext(ext) {
            return convert_qmc(src, opts, res);
        }
        if crate::kwm::is_kwm_ext(ext) {
            return convert_kwm(src, opts, res);
        }
    }

    let mut file = File::open(src)?;
    let size = file.metadata()?.len();
    let ncm = ncm::inspect(&mut file, size)?;

    if let Some(m) = &ncm.meta {
        res.title = m.title.clone();
        res.artist = m.artist.clone();
        res.album = m.album.clone();
    }
    res.format = Some(ncm.format.ext().to_string());

    let out_dir = match &opts.out_dir {
        Some(d) => d.clone(),
        None => src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(".")),
    };
    if opts.out_dir.is_some() {
        fs::create_dir_all(&out_dir)?;
    }

    let fallback = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let base = naming::build_base_name(&res.artist, &res.title, &res.album, &opts.template, &fallback);
    let ext = format!(".{}", ncm.format.ext());
    let plain_path = out_dir.join(format!("{}{}", base, ext));

    let lrc = find_lrc(src, &res.title);
    res.has_lrc = lrc.is_some();

    let final_path = match opts.overwrite {
        Overwrite::Skip => {
            if plain_path.exists() {
                res.status = Status::Skipped;
                // 跳过也要把已存在的文件路径带回去,详情里才能显示/试听
                res.out = Some(plain_path.to_string_lossy().to_string());
                return Ok(());
            }
            plain_path
        }
        Overwrite::Overwrite => plain_path,
        Overwrite::Rename => naming::unique_path(&out_dir, &base, &ext),
    };

    let tmp_path = PathBuf::from(format!("{}.part", final_path.display()));
    let mut out = File::create(&tmp_path)?;
    if let Err(e) = write_audio(&mut file, &ncm, &mut out, res) {
        drop(out);
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    out.flush()?;
    drop(out);

    fs::rename(&tmp_path, &final_path)?;
    res.out = Some(final_path.to_string_lossy().to_string());

    // 歌词:只在输出到别的目录时复制
    if opts.copy_lrc {
        if let Some(lrc_src) = lrc {
            let same_dir = src.parent().map(|p| p == out_dir).unwrap_or(false);
            if !same_dir {
                let stem = final_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| base.clone());
                let target = out_dir.join(format!("{}.lrc", stem));
                if fs::copy(&lrc_src, &target).is_ok() {
                    res.lrc_copied = true;
                }
            }
        }
    }

    Ok(())
}

/// QQ 音乐格式:.qmc* / .mflac / .mgg
///
/// 和 ncm 不同:音频里本来就带着标签,所以这里**不重写标签**,只解密;
/// 标签读出来只用于命名和界面展示。
fn convert_qmc(src: &Path, opts: &ConvertOptions, res: &mut ConvertResult) -> Result<(), NcmError> {
    use crate::qmc;

    let data = fs::read(src)?;
    if data.len() < 16 {
        return Err(NcmError::Corrupt("文件太小".into()));
    }
    let ext_in = src
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let is_v1 = matches!(
        ext_in.as_str(),
        "qmc" | "qmc0" | "qmc2" | "qmc3" | "qmcflac" | "qmcogg"
    );
    let footer = qmc::detect_footer(&data);

    let (cipher, audio_len) = if is_v1 {
        let len = match &footer {
            qmc::Footer::V1 { key_size } => data.len().saturating_sub(4 + key_size),
            _ => data.len(),
        };
        (qmc::Cipher::static_cipher(), len)
    } else {
        match &footer {
            qmc::Footer::QTag { ekey } => {
                let meta_size =
                    u32::from_be_bytes(data[data.len() - 8..data.len() - 4].try_into().unwrap())
                        as usize;
                (
                    qmc::Cipher::from_ekey(ekey).map_err(NcmError::Corrupt)?,
                    data.len().saturating_sub(8 + meta_size),
                )
            }
            qmc::Footer::V1 { key_size } => {
                let start = data.len().saturating_sub(4 + key_size);
                let ekey = qmc::base64_encode(&data[start..data.len() - 4]);
                (
                    qmc::Cipher::from_ekey(&ekey).map_err(NcmError::Corrupt)?,
                    start,
                )
            }
            qmc::Footer::Musicex { .. } => return Err(NcmError::NeedOnline),
            qmc::Footer::Unknown => return Err(NcmError::Key),
        }
    };

    let mut audio = data[..audio_len].to_vec();
    cipher.decrypt(0, &mut audio);

    let fmt = qmc::detect_audio(&audio).unwrap_or_else(|| qmc::fallback_ext(&ext_in));

    finish_plain(src, opts, res, &audio, fmt)
}

/// 酷我音乐 .kwm
fn convert_kwm(src: &Path, opts: &ConvertOptions, res: &mut ConvertResult) -> Result<(), NcmError> {
    let data = fs::read(src)?;
    let d = crate::kwm::decrypt(&data).map_err(NcmError::Corrupt)?;
    finish_plain(src, opts, res, &d.payload, &d.container)
}

/// qmc / kwm 共用的收尾:读标签 → 命名 → 落盘 → 复制歌词
fn finish_plain(
    src: &Path,
    opts: &ConvertOptions,
    res: &mut ConvertResult,
    audio: &[u8],
    fmt: &str,
) -> Result<(), NcmError> {
    use crate::qmc;

    res.format = Some(fmt.to_string());
    let tags = qmc::read_tags(audio);
    res.title = tags.title.clone().unwrap_or_default();
    res.artist = tags.artist.clone().unwrap_or_default();
    res.album = tags.album.clone().unwrap_or_default();

    let out_dir = match &opts.out_dir {
        Some(d) => d.clone(),
        None => src
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    if opts.out_dir.is_some() {
        fs::create_dir_all(&out_dir)?;
    }

    let fallback = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let base = naming::build_base_name(&res.artist, &res.title, &res.album, &opts.template, &fallback);
    let ext = format!(".{}", fmt);
    let plain_path = out_dir.join(format!("{}{}", base, ext));

    let lrc = find_lrc(src, &res.title);
    res.has_lrc = lrc.is_some();

    let final_path = match opts.overwrite {
        Overwrite::Skip => {
            if plain_path.exists() {
                res.status = Status::Skipped;
                res.out = Some(plain_path.to_string_lossy().to_string());
                return Ok(());
            }
            plain_path
        }
        Overwrite::Overwrite => plain_path,
        Overwrite::Rename => naming::unique_path(&out_dir, &base, &ext),
    };

    let tmp_path = PathBuf::from(format!("{}.part", final_path.display()));
    fs::write(&tmp_path, audio)?;
    fs::rename(&tmp_path, &final_path)?;
    res.out = Some(final_path.to_string_lossy().to_string());

    if opts.copy_lrc {
        if let Some(lrc_src) = lrc {
            let same_dir = src.parent().map(|p| p == out_dir).unwrap_or(false);
            if !same_dir {
                let stem = final_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| base.clone());
                let target = out_dir.join(format!("{}.lrc", stem));
                if fs::copy(&lrc_src, &target).is_ok() {
                    res.lrc_copied = true;
                }
            }
        }
    }

    Ok(())
}

/// 转换单个文件(不返回 Err,失败体现在结果里)
pub fn convert_one(src: &Path, opts: &ConvertOptions) -> ConvertResult {
    let t0 = Instant::now();
    let mut res = ConvertResult::new(src);
    if let Err(e) = convert_inner(src, opts, &mut res) {
        res.status = Status::Failed;
        let (code, msg) = err_code(&e);
        res.error_code = Some(code);
        res.error_msg = Some(msg);
    }
    res.ms = t0.elapsed().as_millis();
    res
}

enum BatchEvent {
    Start(usize),
    Result(ConvertResult),
}

/// 并发跑一批。结果通过回调逐个吐出(顺序不保证);`cancel` 置位后不再领取新文件。
pub fn run_batch<S, F>(
    files: &[PathBuf],
    opts: &ConvertOptions,
    concurrency: usize,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    mut on_start: S,
    mut on_result: F,
) where
    S: FnMut(&Path),
    F: FnMut(ConvertResult),
{
    let workers = concurrency.max(1).min(16).min(files.len().max(1));
    let cursor = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<BatchEvent>();

    std::thread::scope(|s| {
        for _ in 0..workers {
            let tx = tx.clone();
            let cursor = &cursor;
            let cancel = cancel.clone();
            s.spawn(move || loop {
                if cancel
                    .as_ref()
                    .map(|c| c.load(Ordering::SeqCst))
                    .unwrap_or(false)
                {
                    break;
                }
                let i = cursor.fetch_add(1, Ordering::SeqCst);
                if i >= files.len() {
                    break;
                }
                if tx.send(BatchEvent::Start(i)).is_err() {
                    break;
                }
                let r = convert_one(&files[i], opts);
                if tx.send(BatchEvent::Result(r)).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        for ev in rx {
            match ev {
                BatchEvent::Start(i) => on_start(&files[i]),
                BatchEvent::Result(r) => on_result(r),
            }
        }
    });
}
